//! E2E PBT transitions, file-per-variant.
//!
//! Each transition kind owns its data + behaviour (preconditions,
//! ref-state apply, async SUT apply, expected SQL budget) in its own
//! file. The `declare_e2e_transitions!` macro generates the dispatch
//! enum, the trait impls on the enum, the `From<Variant> for Enum`
//! conversions, and the `aggregate_transitions(state)` strategy
//! aggregator used by proptest.
//!
//! Adding a transition = create one file under this directory + add
//! one line to the macro invocation below.
//!
//! # Authoring rule: drive the UI, never the engine
//!
//! `apply_to_sut` for a user-action transition MUST go through the
//! `UserDriver` (clicks, keystrokes, leader chords). It must NOT call
//! `engine.execute_op("navigation", ...)` or
//! `engine.ui_state().set_focus(...)`. Two architecture tests in
//! `crates/holon-architecture-tests/tests/architecture_rules.rs`
//! (`no_direct_focus_mutation`, `no_navigation_execute_op_in_tests`)
//! enforce this mechanically; the rule itself:
//!
//! - Real users cannot mutate engine state. Anything reachable from
//!   `apply_to_sut` should be reachable from a real keyboard or pointer.
//! - Bypasses (silently mirroring focus, executing a navigation op
//!   without a chord) hide regressions in the keyboard pipeline,
//!   chord resolution, focus-pin reconciliation, and the renderer's
//!   selectable registry. Catching those is the entire point.
//! - When the production code lacks a binding for an action, ADD the
//!   binding (in `frontends/<frontend>/config/keybindings.yaml` or
//!   the analogous registry), do not paper over it in the test.
//!
//! Setup helpers (loading fixtures, seeding files, harness DI wiring)
//! are exempt — they aren't simulating a user action.

pub mod add_peer;
mod apply_mutation;
mod arrow_navigate;
mod bulk_external_add;
mod click_block;
mod concurrent_mutations;
mod concurrent_schema_init;
mod create_directory;
mod create_document;
mod create_stale_loro;
pub mod delete_backward;
mod drag_drop_block;
mod edit_via_display_tree;
mod edit_via_view_model;
mod emit_mcp_data;
mod expand_toggle;
mod focus_editable_text;
mod git_init;
pub mod indent;
mod jj_git_init;
pub mod join_block;
mod merge_from_peer;
pub mod move_cursor;
pub mod move_down;
pub mod move_up;
mod navigate_back;
mod navigate_focus;
mod navigate_forward;
mod navigate_home;
mod nothing;
pub mod outdent;
mod peer_char_edit;
mod peer_edit;
mod pin_block;
mod press_key;
mod redo;
mod remove_watch;
mod setup_watch;
mod simulate_restart;
pub mod split_block;
mod start_app;
mod switch_view;
mod sync_with_peer;
mod toggle_collapse;
mod toggle_state;
mod trigger_doc_link;
mod trigger_slash_command;
pub mod type_chars;
mod undo_last_mutation;
mod unpin_block;
mod write_org_file;

// Shared layout-PBT variants (delegate to holon-pbt-core + holon-layout-testing).
mod deliver_block_content;
mod switch_view_mode;
mod toggle_drawer;

pub use add_peer::AddPeer;
pub use apply_mutation::ApplyMutation;
pub use arrow_navigate::ArrowNavigate;
pub use bulk_external_add::BulkExternalAdd;
pub use click_block::ClickBlock;
pub use concurrent_mutations::ConcurrentMutations;
pub use concurrent_schema_init::ConcurrentSchemaInit;
pub use create_directory::CreateDirectory;
pub use create_document::CreateDocument;
pub use create_stale_loro::CreateStaleLoro;
pub use delete_backward::DeleteBackward;
pub use drag_drop_block::DragDropBlock;
pub use edit_via_display_tree::EditViaDisplayTree;
pub use edit_via_view_model::EditViaViewModel;
pub use emit_mcp_data::EmitMcpData;
pub use expand_toggle::ExpandToggle;
pub use focus_editable_text::FocusEditableText;
pub use git_init::GitInit;
pub use indent::Indent;
pub use jj_git_init::JjGitInit;
pub use join_block::JoinBlock;
pub use merge_from_peer::MergeFromPeer;
pub use move_cursor::MoveCursor;
pub use move_down::MoveDown;
pub use move_up::MoveUp;
pub use navigate_back::NavigateBack;
pub use navigate_focus::NavigateFocus;
pub use navigate_forward::NavigateForward;
pub use navigate_home::NavigateHome;
pub use nothing::Nothing;
pub use outdent::Outdent;
pub use peer_char_edit::PeerCharEdit;
pub use peer_edit::PeerEdit;
pub use pin_block::PinBlock;
pub use press_key::PressKey;
pub use redo::Redo;
pub use remove_watch::RemoveWatch;
pub use setup_watch::SetupWatch;
pub use simulate_restart::SimulateRestart;
pub use split_block::SplitBlock;
pub use start_app::StartApp;
pub use switch_view::SwitchView;
pub use sync_with_peer::SyncWithPeer;
pub use toggle_state::ToggleState;
pub use trigger_doc_link::TriggerDocLink;
pub use trigger_slash_command::TriggerSlashCommand;
pub use type_chars::TypeChars;
pub use undo_last_mutation::UndoLastMutation;
pub use unpin_block::UnpinBlock;
pub use write_org_file::WriteOrgFile;

pub use deliver_block_content::DeliverBlockContent;
pub use switch_view_mode::SwitchViewMode;
pub use toggle_collapse::ToggleCollapse;
pub use toggle_drawer::ToggleDrawer;

// ── Shared helper types for peer-sync transitions ──────────────────
// These would naturally live in `peer_edit.rs` / `peer_char_edit.rs`
// but they're referenced as field types from BOTH the variant
// definitions and outside callers (sut.rs SutHandle impls), so they
// stay at the module root for ergonomic imports.

use std::hash::{Hash, Hasher};

/// Generate a deterministic, UUID-like stable ID from inputs.
/// Both the reference model and SUT use this to produce identical
/// block IDs for peer-created blocks.
pub fn deterministic_peer_block_id(
    peer_idx: usize,
    parent_stable_id: Option<&str>,
    content: &str,
    seq: usize,
) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    peer_idx.hash(&mut hasher);
    parent_stable_id.hash(&mut hasher);
    content.hash(&mut hasher);
    seq.hash(&mut hasher);
    let h = hasher.finish();
    let hi = (h >> 32) as u32;
    let lo = h as u32;
    format!("peer-{hi:08x}-{lo:08x}-{peer_idx:04x}-{seq:04x}")
}

/// Character-level text operations on a peer's LoroText container.
#[derive(Debug, Clone)]
pub enum TextOp {
    Insert {
        pos_codepoint: usize,
        text: String,
    },
    Delete {
        pos_codepoint: usize,
        len_codepoint: usize,
    },
}

/// Operations that can be performed on a peer's Loro tree.
#[derive(Debug, Clone)]
pub enum PeerEditOp {
    Create {
        parent_stable_id: Option<String>,
        content: String,
        /// Deterministic stable ID from `deterministic_peer_block_id`.
        stable_id: String,
    },
    Update {
        stable_id: String,
        content: String,
    },
    Delete {
        stable_id: String,
    },
}

crate::declare_e2e_transitions! {
    pub enum E2ETransition {
        // ── architecture rule ─────────────────────────────────────
        // Every variant below MUST have a sibling
        // `transitions/<snake_case_name>.rs` file. Enforced by the
        // unit tests in `arch_tests` below the macro invocation.
        ApplyMutation(ApplyMutation),
        ArrowNavigate(ArrowNavigate),
        NavigateBack(NavigateBack),
        BulkExternalAdd(BulkExternalAdd),
        ClickBlock(ClickBlock),
        ConcurrentMutations(ConcurrentMutations),
        CreateDocument(CreateDocument),
        WriteOrgFile(WriteOrgFile),
        CreateDirectory(CreateDirectory),
        DeleteBackward(DeleteBackward),
        DragDropBlock(DragDropBlock),
        EditViaDisplayTree(EditViaDisplayTree),
        EditViaViewModel(EditViaViewModel),
        EmitMcpData(EmitMcpData),
        ExpandToggle(ExpandToggle),
        FocusEditableText(FocusEditableText),
        GitInit(GitInit),
        Indent(Indent),
        JjGitInit(JjGitInit),
        JoinBlock(JoinBlock),
        MoveCursor(MoveCursor),
        MoveDown(MoveDown),
        MoveUp(MoveUp),
        ConcurrentSchemaInit(ConcurrentSchemaInit),
        CreateStaleLoro(CreateStaleLoro),
        StartApp(StartApp),
        Nothing(Nothing),
        NavigateFocus(NavigateFocus),
        NavigateForward(NavigateForward),
        NavigateHome(NavigateHome),
        Outdent(Outdent),
        PinBlock(PinBlock),
        PressKey(PressKey),
        Redo(Redo),
        SimulateRestart(SimulateRestart),
        RemoveWatch(RemoveWatch),
        SplitBlock(SplitBlock),
        SwitchView(SwitchView),
        SetupWatch(SetupWatch),
        ToggleState(ToggleState),
        TriggerDocLink(TriggerDocLink),
        TriggerSlashCommand(TriggerSlashCommand),
        TypeChars(TypeChars),
        UndoLastMutation(UndoLastMutation),
        UnpinBlock(UnpinBlock),
        AddPeer(AddPeer),
        PeerEdit(PeerEdit),
        SyncWithPeer(SyncWithPeer),
        MergeFromPeer(MergeFromPeer),
        PeerCharEdit(PeerCharEdit),
        SwitchViewMode(SwitchViewMode),
        ToggleDrawer(ToggleDrawer),
        ToggleCollapse(ToggleCollapse),
        DeliverBlockContent(DeliverBlockContent),
    }
}

#[cfg(test)]
mod arch_tests {
    /// Each variant in `E2ETransition` must have a sibling
    /// `transitions/<snake_case_name>.rs` file. Catches accidental
    /// drift where a variant lives in `mod.rs` but its impls leak
    /// into another module — defeats the file-per-transition contract.
    #[test]
    fn every_variant_has_a_dedicated_file() {
        let module_source = include_str!("./mod.rs");

        // Extract the body of `pub enum E2ETransition { ... }` inside
        // the `declare_e2e_transitions! { ... }` invocation by tracking
        // brace depth — `rsplit_once('}')` would match the file's final
        // `}` (e.g. the `arch_tests` module close), not the macro body.
        // Use the qualified call form so we don't match earlier doc-comment
        // mentions of the macro name or the test-code references below.
        let after_marker = module_source
            .split_once("crate::declare_e2e_transitions!")
            .expect("crate::declare_e2e_transitions! invocation must exist")
            .1;
        // Skip past the macro's outer `{`, then the enum's inner `{`.
        let mut chars = after_marker.char_indices();
        let mut start = None;
        let mut braces_seen = 0;
        for (idx, c) in chars.by_ref() {
            if c == '{' {
                braces_seen += 1;
                if braces_seen == 2 {
                    start = Some(idx + 1);
                    break;
                }
            }
        }
        let start = start.expect("two opening braces (macro + enum)");
        // Walk forward tracking depth; stop at depth-zero `}`.
        let mut depth: i32 = 1;
        let mut end = None;
        for (idx, c) in chars {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(idx);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end.expect("matching closing brace of enum body");
        let body = &after_marker[start..end];

        let mut variant_names: Vec<String> = Vec::new();
        for raw_line in body.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            let name: String = line
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                variant_names.push(name);
            }
        }

        assert!(
            !variant_names.is_empty(),
            "extracted no variant names from declare_e2e_transitions! body — parser drifted"
        );

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pbt/transitions");
        let mut missing: Vec<String> = Vec::new();
        for name in &variant_names {
            let mut snake = String::new();
            for (i, c) in name.chars().enumerate() {
                if c.is_uppercase() && i > 0 {
                    snake.push('_');
                }
                snake.push(c.to_ascii_lowercase());
            }
            let path = dir.join(format!("{snake}.rs"));
            if !path.exists() {
                missing.push(format!("{name} → expected {}", path.display()));
            }
        }

        assert!(
            missing.is_empty(),
            "{} variant(s) missing dedicated file:\n  {}",
            missing.len(),
            missing.join("\n  ")
        );
    }

    /// Inverse: every `<snake>.rs` file under `transitions/` should
    /// correspond to a variant registered in the macro invocation.
    /// Catches stale variant files left behind after a delete.
    #[test]
    fn every_file_is_registered_as_a_module() {
        let module_source = include_str!("./mod.rs");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pbt/transitions");

        let mut registered_modules: Vec<String> = Vec::new();
        for line in module_source.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("mod ")
                && let Some((name, _)) = rest.split_once(';')
            {
                registered_modules.push(name.to_string());
            }
        }

        let mut orphan_files: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("read transitions dir") {
            let entry = entry.expect("read entry");
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if stem == "mod" {
                continue;
            }
            if !registered_modules.contains(&stem) {
                orphan_files.push(stem);
            }
        }

        assert!(
            orphan_files.is_empty(),
            "{} orphan file(s) under transitions/ not registered in mod.rs:\n  {}",
            orphan_files.len(),
            orphan_files.join("\n  ")
        );
    }
}
