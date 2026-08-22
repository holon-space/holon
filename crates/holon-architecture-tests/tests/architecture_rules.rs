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

use std::collections::BTreeMap;
use std::collections::BTreeSet;
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

/// Calls of `WriteTxn::doc()`, which runs under the held write guard. Pinned
/// per file, so a `txn.doc()` anywhere else still counts as an escape.
const WRITE_TXN_DOC_SITES: &[(&str, usize)] = &[("crates/holon-loro/src/loro_share_backend.rs", 3)];

/// Calls of a `doc()` accessor belonging to another type, with that type's
/// name. Pinned per file, so an escape added alongside them still counts.
const UNRELATED_DOC_ACCESSOR_SITES: &[(&str, usize, &str)] = &[
    ("crates/holon-sharing/src/alias_ledger.rs", 6, "AliasLedger"),
    (
        "crates/holon-loro-testing/src/shadow_mesh.rs",
        8,
        "ShadowDoc",
    ),
];

/// The blessed `LoroDocument::doc()` escapes, per file.
const DOC_ESCAPES: &[(&str, usize)] = &[
    (
        "crates/holon-integration-tests/src/pbt/composed/two_instance.rs",
        3,
    ),
    ("crates/holon-integration-tests/src/pbt/convergence.rs", 1),
    (
        "crates/holon-integration-tests/src/pbt/loro_sync/stub_sut.rs",
        4,
    ),
    (
        "crates/holon-integration-tests/tests/loro_suite/loro_projection_atomic_advance.rs",
        1,
    ),
    ("crates/holon-loro-testing/src/quiescence.rs", 1),
    ("crates/holon-loro-testing/src/sut_loro.rs", 3),
    ("crates/holon-loro/src/block_cell_registry.rs", 1),
    ("crates/holon-loro/src/container_registry.rs", 2),
    ("crates/holon-loro/src/deleted_container_purge.rs", 2),
    ("crates/holon-loro/src/import_atomicity_probe.rs", 7),
    ("crates/holon-loro/src/loro_backend.rs", 8),
    ("crates/holon-loro/src/loro_document.rs", 3),
    ("crates/holon-loro/src/loro_share_backend.rs", 15),
    ("crates/holon-loro/src/loro_sync_controller.rs", 1),
    ("crates/holon-sharing/src/sync.rs", 1),
    ("crates/holon-loro-wiring/src/loro_module.rs", 1),
    ("crates/holon/tests/api_pbt/loro_backend_pbt.rs", 4),
    ("crates/holon/tests/api_suite/loro_backend_pbt.rs", 4),
    ("crates/holon/tests/sync_suite/sync_pbt.rs", 3),
];

/// Every `doc()` call per file, whichever type it belongs to.
fn doc_calls_by_file(root: &Path) -> BTreeMap<String, usize> {
    let mut files = Vec::new();
    rust_sources(&root.join("crates"), &mut files);
    rust_sources(&root.join("frontends"), &mut files);

    let mut found = BTreeMap::new();
    for file in &files {
        let rel = file
            .strip_prefix(root)
            .expect("source under repo root")
            .to_string_lossy()
            .replace('\\', "/");
        // This file names the accessor in prose, on lines the comment filter
        // misses. It has no Loro dependency, so nothing here can be an escape.
        if rel == file!() {
            continue;
        }
        let src = std::fs::read_to_string(file).expect("readable rust source");
        let count: usize = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .map(|l| l.matches(".doc()").count() + l.matches("LoroDocument::doc").count())
            .sum();
        if count > 0 {
            found.insert(rel, count);
        }
    }
    found
}

/// `LoroDocument::doc()` hands the raw `Arc<LoroDoc>` out from under the
/// doc-boundary lock, so every call site can observe a write batch's interior.
/// The blessed set is pinned per file by `DOC_ESCAPES`.
///
/// Matching rule: on non-comment lines of every `.rs` under `crates/` and
/// `frontends/`, each `.doc()` and each `LoroDocument::doc` (the UFCS and
/// method-reference forms) counts as an escape, less the per-file pins in
/// `WRITE_TXN_DOC_SITES` and `UNRELATED_DOC_ACCESSOR_SITES`. Those two pin
/// counts, not files and not receiver names, so an escape added next to a
/// blessed call still lands in the diff. Definitions read `fn doc(` and so
/// never match; a `.doc()` in a trailing `//` comment does match, which
/// over-reports rather than under-reports.
#[test]
fn loro_doc_escapes_match_the_allow_list() {
    let root = repo_root();

    for (file, _, ty) in UNRELATED_DOC_ACCESSOR_SITES {
        let src = std::fs::read_to_string(root.join(file)).expect("readable rust source");
        assert!(
            src.contains("fn doc(&self) -> &LoroDoc"),
            "{file} no longer declares `{ty}::doc(&self) -> &LoroDoc`, so its \
             UNRELATED_DOC_ACCESSOR_SITES pin is stale — re-classify that file's `doc()` calls \
             and re-seed the tables.",
        );
    }

    let expected: BTreeMap<String, usize> = DOC_ESCAPES
        .iter()
        .map(|(f, n)| ((*f).to_string(), *n))
        .collect();
    let calls = doc_calls_by_file(&root);
    let pinned: BTreeMap<&str, usize> = WRITE_TXN_DOC_SITES
        .iter()
        .map(|(f, n)| (*f, *n))
        .chain(
            UNRELATED_DOC_ACCESSOR_SITES
                .iter()
                .map(|(f, n, _)| (*f, *n)),
        )
        .fold(BTreeMap::new(), |mut acc, (f, n)| {
            *acc.entry(f).or_default() += n;
            acc
        });

    let mut diffs = Vec::new();
    for file in expected
        .keys()
        .chain(calls.keys())
        .map(String::as_str)
        .chain(pinned.keys().copied())
        .collect::<BTreeSet<_>>()
    {
        let want = expected.get(file).copied().unwrap_or(0);
        let total = calls.get(file).copied().unwrap_or(0);
        let non_escape = pinned.get(file).copied().unwrap_or(0);
        let Some(got) = total.checked_sub(non_escape) else {
            diffs.push(format!(
                "  {file}: {non_escape} calls pinned as non-escapes but the file has only {total} \
                 `doc()` calls in total"
            ));
            continue;
        };
        if want != got {
            diffs.push(format!(
                "  {file}: allow-listed {want}, found {got} ({total} `doc()` calls, {non_escape} \
                 pinned as non-escapes)"
            ));
        }
    }

    assert!(
        diffs.is_empty(),
        "the `LoroDocument::doc()` escape allow-list is out of date:\n{}\n\nA new escape hands \
         the raw `Arc<LoroDoc>` out from under the doc-boundary lock. Route the read or mutation \
         through `LoroDocument::with_read` / `with_write_origin` instead; if the site genuinely \
         belongs outside the lock (a long-lived transport/subscription, or a cell backing's \
         retained container handle), extend DOC_ESCAPES in {} with a comment saying which of \
         those it is. A count that dropped is just a removed escape — lower the entry. If the \
         call is not `LoroDocument::doc()` at all, it belongs in WRITE_TXN_DOC_SITES (a \
         `WriteTxn` under the held guard) or UNRELATED_DOC_ACCESSOR_SITES (another type's \
         accessor) instead.\n",
        diffs.join("\n"),
        file!(),
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
