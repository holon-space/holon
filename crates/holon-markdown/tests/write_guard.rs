//! Tier R/O write-back guard proofs: a foreign vault is NEVER written.
//!
//! ADR 0025 — a foreign file is not produced by a Holon op, so no op can ground
//! a write to it. R/O makes this unconditional. These tests prove the three
//! layers that enforce it: the controller-level [`ReadOnlyWriteGuard`], the
//! adapter's `writeback_drops` refusal, and the render panic.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_core::file_format::FileFormatAdapter;
use holon_markdown::LogseqMarkdownAdapter;
use holon_markdown::ObsidianMarkdownAdapter;
use holon_markdown::ReadOnlyWriteGuard;

#[test]
fn read_only_guard_vetoes_every_write() {
    let guard = ReadOnlyWriteGuard::read_only();
    assert!(!guard.may_write(Path::new("/vault/pages/Compat.md")));
}

#[test]
fn writable_guard_permits_writes() {
    let guard = ReadOnlyWriteGuard::writable();
    assert!(guard.may_write(Path::new("/vault/notes/todo.org")));
}

#[test]
#[should_panic(expected = "refused write to read-only")]
fn checked_write_panics_on_read_only_vault() {
    let guard = ReadOnlyWriteGuard::read_only();
    // A mis-routed write must fail LOUD at the fs.write call site.
    let _ = guard.checked(Path::new("/vault/pages/Compat.md"), "clobbering bytes");
}

#[test]
fn logseq_adapter_refuses_writeback_drop_verdict() {
    let a = LogseqMarkdownAdapter::new();
    let removals = std::collections::HashSet::new();
    let err = a
        .writeback_drops(
            Path::new("/v/p/Compat.md"),
            "- a\n",
            "- b\n",
            &[],
            &removals,
            Path::new("/v"),
        )
        .unwrap_err();
    assert!(err.to_string().contains("read-only") || err.to_string().contains("Tier R/O"));
}

#[test]
fn obsidian_adapter_refuses_writeback_drop_verdict() {
    let a = ObsidianMarkdownAdapter::new();
    let removals = std::collections::HashSet::new();
    let err = a
        .writeback_drops(
            Path::new("/v/Note.md"),
            "x",
            "y",
            &[],
            &removals,
            Path::new("/v"),
        )
        .unwrap_err();
    assert!(err.to_string().contains("read-only") || err.to_string().contains("Tier R/O"));
}

/// One call per adapter and render entry point; there is no input space, the
/// adapters are read-only for every input.
#[test]
fn every_render_entry_point_is_an_error_naming_adapter_and_path() {
    let doc = Block::new_text(
        EntityUri::file("Compat.md"),
        EntityUri::no_parent(),
        "Compat",
    );
    let uri = EntityUri::file("Compat.md");
    let path = Path::new("/v/Compat.md");
    let adapters: [(&str, Box<dyn FileFormatAdapter>); 2] = [
        (
            "LogseqMarkdownAdapter",
            Box::new(LogseqMarkdownAdapter::new()),
        ),
        (
            "ObsidianMarkdownAdapter",
            Box::new(ObsidianMarkdownAdapter::new()),
        ),
    ];
    for (name, adapter) in &adapters {
        let errors = [
            adapter.render_document(&doc, &[], path, &uri).unwrap_err(),
            adapter.render_blocks(&[], path, &uri).unwrap_err(),
        ];
        for e in errors {
            let message = e.to_string();
            for needle in [name, "read-only", "/v/Compat.md"] {
                assert!(message.contains(needle), "{needle} not in {message}");
            }
        }
    }
}
