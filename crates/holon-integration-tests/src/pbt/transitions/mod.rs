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
//! - Bypasses (silently mirroring focus, executing a navigation op without a
//!   chord) hide regressions in the keyboard pipeline, chord resolution,
//!   focus-pin reconciliation, and the renderer's selectable registry. Catching
//!   those is the entire point.
//! - When the production code lacks a binding for an action, ADD the binding
//!   (in `frontends/<frontend>/config/keybindings.yaml` or the analogous
//!   registry), do not paper over it in the test.
//!
//! Setup helpers (loading fixtures, seeding files, harness DI wiring)
//! are exempt — they aren't simulating a user action.

/// Model the click that `dispatch_block_op_via_chord` performs before
/// pressing a chord: focus the block (editor focus, no nav-history entry —
/// same semantics as `ClickBlock`'s non-navigating branch) and open its
/// editor at end-of-text (the plain-click caret-seed default). Chord-driven
/// structural ops (Indent / Outdent / MoveUp / MoveDown) call this from
/// their ref applies so `inv-focus-matches-ref` and editor-state invariants
/// see the same focus move the SUT's real input pipeline produced.
///
/// When the block's editor is ALREADY active, no driver clicks: the GPUI
/// driver skips its click-to-focus (the chord goes to the focused editor
/// directly, like a real user) and headless drivers never click at all —
/// so the ref must leave the editor and its caret untouched. Re-seeding
/// the caret to end-of-text here diverged from every SUT after
/// `SplitBlock → <chord op>` on the freshly-focused new block
/// (`inv-editor-caret/mirror`: ref end-of-text vs SUT 0).
pub fn model_chord_click_focus<
    R: holon_pbt_core::capabilities::RefBlockTree
        + holon_pbt_core::capabilities::RefBlockTreeMut
        + holon_pbt_core::capabilities::RefFocus
        + holon_pbt_core::capabilities::RefFocusMut
        + holon_pbt_core::capabilities::RefEditorMirrorMut,
>(
    block_id: &holon_api::EntityUri,
    state: &mut R,
) {
    use holon_pbt_core::capabilities::CapCursor;
    use holon_pbt_core::capabilities::CapRegion;
    use holon_pbt_core::capabilities::commit_active_editor_if_dirty;
    if state.active_editor_block().as_ref() == Some(block_id) {
        return;
    }
    // Click-away BLURS the previously focused editor; in SqlOnly prod's
    // on_blur commits its user-authored pending text. Dirty-gated: a clean
    // mirror that merely diverged from block.content is stale against an
    // external change and must NOT be committed (prod's editor would have
    // been refreshed by the data subscription).
    commit_active_editor_if_dirty(state);
    let content = state
        .block_content(block_id)
        .unwrap_or_default()
        .to_string();
    let caret = content.len();
    state.set_focus(CapRegion::Main, block_id.clone(), CapCursor::default());
    state.open_active_editor(block_id.clone(), content, caret);
}

mod advance_day;
pub mod apply_mutation;
mod arrow_navigate;
pub(crate) mod block_to_page;
pub mod bulk_external_add;
pub mod click_block;
mod concurrent_schema_init;
mod create_block_under_focus;
mod create_directory;
mod create_document;
pub mod delete_backward;
mod delete_document;
mod drag_drop_block;
mod emit_mcp_data;
mod epoch_flip_rejected;
mod expand_toggle;
pub mod external_write_while_focused;
pub mod focus_editable_text;
mod git_init;
pub mod indent;
mod instantiate_template;
mod jj_git_init;
pub mod join_block;
pub mod move_cursor;
pub mod move_down;
pub mod move_up;
mod navigate_back;
mod navigate_focus;
mod navigate_forward;
mod navigate_home;
mod nothing;
pub mod outdent;
mod pin_block;
mod press_key;
mod redo;
mod remove_watch;
pub mod set_edge_field;
mod setup_watch;
mod simulate_restart;
pub mod split_block;
pub mod stale_external_rewrite;
pub(crate) mod start_app;
mod switch_view;
mod toggle_collapse;
pub mod toggle_state;
pub mod trigger_slash_command;
pub mod type_chars;
mod undo_last_mutation;
mod unpin_block;
pub(crate) mod write_org_file;

// Shared layout-PBT variants (delegate to holon-pbt-core +
// holon-layout-testing).
mod deliver_block_content;
mod switch_view_mode;
mod toggle_drawer;

pub use advance_day::AdvanceDay;
pub use apply_mutation::ApplyMutation;
pub use arrow_navigate::ArrowNavigate;
pub use block_to_page::BlockToPage;
pub use bulk_external_add::BulkExternalAdd;
pub use click_block::ClickBlock;
pub use concurrent_schema_init::ConcurrentSchemaInit;
pub use create_block_under_focus::CreateBlockUnderFocus;
pub use create_directory::CreateDirectory;
pub use create_document::CreateDocument;
pub use delete_backward::DeleteBackward;
pub use delete_document::DeleteDocument;
pub use deliver_block_content::DeliverBlockContent;
pub use drag_drop_block::DragDropBlock;
pub use emit_mcp_data::EmitMcpData;
pub use epoch_flip_rejected::EpochFlipRejected;
pub use expand_toggle::ExpandToggle;
pub use external_write_while_focused::ExternalWriteWhileFocused;
pub use focus_editable_text::FocusEditableText;
pub use git_init::GitInit;
// The peer-sync transitions (`AddPeer`, `PeerEdit`, `PeerCharEdit`,
// `MergeFromPeer`, `SyncWithPeer`, `CreateStaleLoro`) and their shared helper
// `deterministic_peer_block_id` are co-located in `holon-loro-testing` /
// `holon-pbt-core` (Phase-1a Step 4). Re-exported here so the
// `declare_e2e_transitions!` enum below (the central assembler) names them
// unchanged; `deterministic_peer_block_id` now lives in
// `holon_pbt_core::capabilities`.
pub use holon_loro_testing::transitions::{
    AddPeer, CreateStaleLoro, MergeFromPeer, PeerCharEdit, PeerEdit, SyncWithPeer,
};
pub use indent::Indent;
pub use instantiate_template::InstantiateTemplate;
pub use jj_git_init::JjGitInit;
pub use join_block::JoinBlock;
pub use move_cursor::MoveCursor;
pub use move_down::MoveDown;
pub use move_up::MoveUp;
pub use navigate_back::NavigateBack;
pub use navigate_focus::NavigateFocus;
pub use navigate_forward::NavigateForward;
pub use navigate_home::NavigateHome;
pub use nothing::Nothing;
pub use outdent::Outdent;
pub use pin_block::PinBlock;
pub use press_key::PressKey;
pub use redo::Redo;
pub use remove_watch::RemoveWatch;
pub use set_edge_field::SetEdgeField;
pub use setup_watch::SetupWatch;
pub use simulate_restart::SimulateRestart;
pub use split_block::SplitBlock;
pub use stale_external_rewrite::StaleExternalRewrite;
pub use start_app::StartApp;
pub use switch_view::SwitchView;
pub use switch_view_mode::SwitchViewMode;
pub use toggle_collapse::ToggleCollapse;
pub use toggle_drawer::ToggleDrawer;
pub use toggle_state::ToggleState;
pub use trigger_slash_command::TriggerSlashCommand;
pub use type_chars::TypeChars;
pub use undo_last_mutation::UndoLastMutation;
pub use unpin_block::UnpinBlock;
pub use write_org_file::WriteOrgFile;

crate::declare_e2e_transitions! {
    pub enum E2ETransition {
        // ── architecture rule ─────────────────────────────────────
        // Every variant below MUST have a sibling
        // `transitions/<snake_case_name>.rs` file. Enforced by the
        // unit tests in `arch_tests` below the macro invocation.
        AdvanceDay(AdvanceDay),
        ApplyMutation(ApplyMutation),
        ArrowNavigate(ArrowNavigate),
        BlockToPage(BlockToPage),
        NavigateBack(NavigateBack),
        BulkExternalAdd(BulkExternalAdd),
        StaleExternalRewrite(StaleExternalRewrite),
        ExternalWriteWhileFocused(ExternalWriteWhileFocused),
        ClickBlock(ClickBlock),
        CreateBlockUnderFocus(CreateBlockUnderFocus),
        CreateDocument(CreateDocument),
        WriteOrgFile(WriteOrgFile),
        CreateDirectory(CreateDirectory),
        DeleteBackward(DeleteBackward),
        DeleteDocument(DeleteDocument),
        DragDropBlock(DragDropBlock),
        EmitMcpData(EmitMcpData),
        EpochFlipRejected(EpochFlipRejected),
        ExpandToggle(ExpandToggle),
        FocusEditableText(FocusEditableText),
        GitInit(GitInit),
        Indent(Indent),
        InstantiateTemplate(InstantiateTemplate),
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
        SetEdgeField(SetEdgeField),
        SplitBlock(SplitBlock),
        SwitchView(SwitchView),
        SetupWatch(SetupWatch),
        ToggleState(ToggleState),
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
        // The peer-sync transitions were co-located into `holon-loro-testing`
        // (Phase-1a Step 4 — see the `pub use holon_loro_testing::transitions::…`
        // re-export above). Their dedicated files live in that crate's
        // `src/transitions/` dir, not here. The file-per-transition invariant
        // still holds — it just spans the crate seam, so these variants are
        // searched for in the co-located crate instead.
        let loro_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../holon-loro-testing/src/transitions");
        let relocated_to_loro: &[&str] = &[
            "AddPeer",
            "CreateStaleLoro",
            "MergeFromPeer",
            "PeerCharEdit",
            "PeerEdit",
            "SyncWithPeer",
        ];
        let mut missing: Vec<String> = Vec::new();
        for name in &variant_names {
            let mut snake = String::new();
            for (i, c) in name.chars().enumerate() {
                if c.is_uppercase() && i > 0 {
                    snake.push('_');
                }
                snake.push(c.to_ascii_lowercase());
            }
            let search_dir = if relocated_to_loro.contains(&name.as_str()) {
                &loro_dir
            } else {
                &dir
            };
            let path = search_dir.join(format!("{snake}.rs"));
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
            // Strip an optional visibility modifier (`pub`, `pub(crate)`,
            // `pub(super)`, `pub(in path)`) before the `mod` keyword, so a
            // `pub(crate) mod start_app;` is recognized like a plain `mod`.
            let mut s = line.trim();
            if let Some(rest) = s.strip_prefix("pub") {
                s = rest.trim_start();
                if s.starts_with('(')
                    && let Some(close) = s.find(')')
                {
                    s = s[close + 1..].trim_start();
                }
            }
            if let Some(rest) = s.strip_prefix("mod ")
                && let Some((name, _)) = rest.split_once(';')
            {
                registered_modules.push(name.trim().to_string());
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

/// PCG-3 guard: each transition's `required_caps()` matches the capability its
/// `TransitionImpl` is bound on, so the cap gate (PCG-2) admits a transition
/// into a composed `CapMap`'s alphabet only when `apply_to_sut`'s cap is
/// present — never generating one that would panic on an absent `expect`. For
/// the 43 fine-grained-bound transitions the body is type-guaranteed to use
/// only that cap; the 5 peer ops defer to PCG-4 (`SutLoro` isn't dyn-compatible
/// yet, and they're already wiring-gated on `HasStorage(Loro)`);
/// `Nothing`/`DeliverBlockContent` use no cap.
#[cfg(test)]
mod required_caps_guard {
    use holon_pbt_core::TransitionFactory;
    use holon_pbt_core::composition::CapId;

    use super::*;
    use crate::pbt::reference_state::ReferenceState;

    fn caps<T: TransitionFactory<ReferenceState>>() -> Vec<CapId> {
        T::required_caps()
    }

    #[test]
    fn required_caps_match_transition_impl_bounds() {
        use holon_frontend::pbt_caps as fe;
        use holon_pbt_core::capabilities as lc;
        use holon_pbt_core::capabilities as c;

        macro_rules! one {
            ($t:ty, $cap:path) => {
                assert_eq!(
                    caps::<$t>(),
                    vec![CapId::of::<dyn $cap>()],
                    concat!(
                        stringify!($t),
                        ": required_caps must be exactly [",
                        stringify!($cap),
                        "]"
                    )
                );
            };
        }
        macro_rules! none {
            ($t:ty) => {
                assert!(
                    caps::<$t>().is_empty(),
                    concat!(stringify!($t), ": required_caps must be empty")
                );
            };
        }

        // BlockTreeWrite
        // SplitBlock omitted: migrated to `cap_transition!`, which single-sources the
        // cap, so its required_caps and `S: SutBlockTreeWrite` bound cannot drift —
        // no guard entry needed. (The drop-out the macro is designed to produce.)
        one!(JoinBlock, c::SutBlockTreeWrite);
        one!(Indent, c::SutBlockTreeWrite);
        one!(Outdent, c::SutBlockTreeWrite);
        one!(MoveUp, c::SutBlockTreeWrite);
        one!(MoveDown, c::SutBlockTreeWrite);
        // EditorMirrorWrite
        one!(TypeChars, c::SutEditorMirrorWrite);
        one!(DeleteBackward, c::SutEditorMirrorWrite);
        one!(MoveCursor, c::SutEditorMirrorWrite);
        // FocusWrite
        one!(NavigateFocus, c::SutFocusWrite);
        one!(FocusEditableText, c::SutFocusWrite);
        // NavHistoryWrite
        one!(NavigateHome, c::SutNavHistoryWrite);
        // NavHistoryDrive
        one!(NavigateBack, c::SutNavHistoryDrive);
        one!(NavigateForward, c::SutNavHistoryDrive);
        one!(PinBlock, c::SutNavHistoryDrive);
        one!(UnpinBlock, c::SutNavHistoryDrive);
        // WatchRegister
        one!(SetupWatch, c::SutWatchRegister);
        one!(RemoveWatch, c::SutWatchRegister);
        // ViewControl / McpEmit / HistoryWrite
        one!(SwitchView, c::SutViewControl);
        one!(EmitMcpData, c::SutMcpEmit);
        one!(Redo, c::SutHistoryWrite);
        one!(UndoLastMutation, c::SutHistoryWrite);
        // BlockInteract
        one!(ClickBlock, c::SutBlockInteract);
        one!(DragDropBlock, c::SutBlockInteract);
        one!(ExpandToggle, c::SutBlockInteract);
        one!(PressKey, c::SutBlockInteract);
        one!(SwitchViewMode, c::SutBlockInteract);
        one!(ToggleCollapse, c::SutBlockInteract);
        one!(ToggleDrawer, c::SutBlockInteract);
        one!(TriggerSlashCommand, c::SutBlockInteract);
        // ArrowNavigate (holon-frontend)
        one!(ArrowNavigate, fe::SutArrowNavigate);
        // Mutate (test-local)
        one!(ToggleState, lc::SutMutate);
        // ApplyMutation is now SOURCE-ROUTED via `SutApplyMutation` (one transition,
        // source as a shrinkable axis), so its dispatch trait and its GATE cap
        // legitimately differ: the gate names `SutLoro` (the implemented
        // composed arm — LoroPeer), while dispatch goes through
        // `SutApplyMutation`. The "one fine-grained cap == bound" drift
        // guard therefore tracks the GATE cap here, not the dispatch trait.
        one!(ApplyMutation, c::SutLoro);
        // BulkExternalAdd still binds the `SutSeamMutate` seam cap (pre-existing).
        one!(BulkExternalAdd, lc::SutSeamMutate);
        // FixtureFs (test-local)
        one!(CreateDirectory, lc::SutFixtureFs);
        one!(CreateStaleLoro, lc::SutFixtureFs);
        one!(GitInit, lc::SutFixtureFs);
        one!(JjGitInit, lc::SutFixtureFs);
        one!(WriteOrgFile, lc::SutFixtureFs);
        // AppLifecycle (test-local)
        one!(ConcurrentSchemaInit, lc::SutAppLifecycle);
        one!(CreateDocument, lc::SutAppLifecycle);
        one!(DeleteDocument, lc::SutAppLifecycle);
        one!(EpochFlipRejected, lc::SutAppLifecycle);
        one!(SimulateRestart, lc::SutAppLifecycle);
        one!(StartApp, lc::SutAppLifecycle);

        // Loro peer ops — PCG-4 flipped `SutLoro` to `&self` + dyn-compatible, so `dyn
        // SutLoro`/`CapId::of` now exist. Also wiring-gated on `HasStorage(Loro)`.
        one!(AddPeer, c::SutLoro);
        one!(PeerEdit, c::SutLoro);
        one!(PeerCharEdit, c::SutLoro);
        one!(SyncWithPeer, c::SutLoro);
        one!(MergeFromPeer, c::SutLoro);
        // No SUT capability needed.
        // Nothing omitted: migrated to `cap_transition!` (no-cap form) —
        // single-sourced.
        none!(DeliverBlockContent);
    }
}
