//! Compile-time proof of Stage 2 / B2 cross-medium reuse.
//!
//! The E2E transition structs that were narrowed to fine-grained
//! capabilities implement `holon_pbt_core::TransitionImpl<Ref, S>` for
//! **any** SUT supplying that capability — not only `E2ESut` via the
//! coarse `SutHandle` bundle. This test defines a mock SUT that
//! implements *only* `SutEditorMirrorWrite` (and is deliberately NOT a
//! `SutHandle`) and asserts, at compile time, that `TypeChars`,
//! `DeleteBackward`, and `MoveCursor` can drive it. If the narrowing
//! ever regresses to `S: SutHandle`, this test stops compiling.

#![cfg(feature = "pbt")]

use holon_integration_tests::pbt::ReferenceState;
use holon_integration_tests::pbt::transitions::{
    DeleteBackward, Indent, JoinBlock, MoveCursor, MoveDown, MoveUp, Outdent, SplitBlock, TypeChars,
};
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::capabilities::{CapBlockId, SutBlockTreeWrite, SutEditorMirrorWrite};

/// A SUT that supplies ONLY the editor-mirror-write capability. It is
/// not an `E2ESut` and does not implement `SutHandle`.
#[derive(Default)]
struct MiniEditorSut {
    text: String,
}

impl SutEditorMirrorWrite for MiniEditorSut {
    async fn apply_type_chars(&mut self, text: &str) {
        self.text.push_str(text);
    }
    async fn apply_delete_backward(&mut self, count: usize) {
        for _ in 0..count {
            self.text.pop();
        }
    }
    async fn apply_move_cursor(&mut self, byte_position: usize) {
        // No cursor model in the mock; just observe the call.
        let _ = byte_position;
    }
}

/// A SUT that supplies ONLY the block-tree-write capability. It is not an
/// `E2ESut` and does not implement `SutHandle` — proving the block-tree
/// transitions are narrowed to `S: SutBlockTreeWrite`, not the coarse
/// `SutHandle` bundle. If the narrowing ever regresses, this stops
/// compiling.
#[derive(Default)]
struct MiniBlockTreeSut {
    last_op: Option<String>,
}

impl SutBlockTreeWrite for MiniBlockTreeSut {
    async fn apply_split_block(&mut self, id: &CapBlockId, position: usize) {
        self.last_op = Some(format!("split {id} @ {position}"));
    }
    async fn apply_join_block(&mut self, id: &CapBlockId) {
        self.last_op = Some(format!("join {id}"));
    }
    async fn apply_indent(&mut self, id: &CapBlockId) {
        self.last_op = Some(format!("indent {id}"));
    }
    async fn apply_outdent(&mut self, id: &CapBlockId) {
        self.last_op = Some(format!("outdent {id}"));
    }
    async fn apply_move_up(&mut self, id: &CapBlockId) {
        self.last_op = Some(format!("move_up {id}"));
    }
    async fn apply_move_down(&mut self, id: &CapBlockId) {
        self.last_op = Some(format!("move_down {id}"));
    }
}

/// Compiles only if `T` implements `TransitionImpl` for `Sut`.
fn assert_cross_medium<Sut, T: TransitionImpl<ReferenceState, Sut>>() {}

#[test]
fn narrowed_editor_transitions_run_on_non_e2e_sut() {
    assert_cross_medium::<MiniEditorSut, TypeChars>();
    assert_cross_medium::<MiniEditorSut, DeleteBackward>();
    assert_cross_medium::<MiniEditorSut, MoveCursor>();
}

#[test]
fn narrowed_block_tree_transitions_run_on_non_e2e_sut() {
    assert_cross_medium::<MiniBlockTreeSut, SplitBlock>();
    assert_cross_medium::<MiniBlockTreeSut, JoinBlock>();
    assert_cross_medium::<MiniBlockTreeSut, Indent>();
    assert_cross_medium::<MiniBlockTreeSut, Outdent>();
    assert_cross_medium::<MiniBlockTreeSut, MoveUp>();
    assert_cross_medium::<MiniBlockTreeSut, MoveDown>();
}
