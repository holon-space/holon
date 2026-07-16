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
//!
//! @pbt kind harness
//! @pbt covers cross-medium-cap-reuse(compile-time) — Stage2/B2 compile-time
//! proof of cross-medium transition reuse @pbt overlaps arch-lint —
//! compile-only; candidate to replace with a module-boundary lint

#![cfg(feature = "pbt")]

use holon_integration_tests::pbt::ReferenceState;
use holon_integration_tests::pbt::transitions::DeleteBackward;
use holon_integration_tests::pbt::transitions::Indent;
use holon_integration_tests::pbt::transitions::JoinBlock;
use holon_integration_tests::pbt::transitions::MoveCursor;
use holon_integration_tests::pbt::transitions::MoveDown;
use holon_integration_tests::pbt::transitions::MoveUp;
use holon_integration_tests::pbt::transitions::Outdent;
use holon_integration_tests::pbt::transitions::SplitBlock;
use holon_integration_tests::pbt::transitions::TypeChars;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::capabilities::CapBlockId;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::capabilities::SutEditorMirrorWrite;

/// A SUT that supplies ONLY the editor-mirror-write capability. It is
/// not an `E2ESut` and does not implement `SutHandle`.
#[derive(Default)]
struct MiniEditorSut {
    text: std::cell::RefCell<String>,
}

#[async_trait::async_trait(?Send)]
impl SutEditorMirrorWrite for MiniEditorSut {
    async fn apply_type_chars(&self, text: &str) {
        self.text.borrow_mut().push_str(text);
    }
    async fn apply_delete_backward(&self, count: usize) {
        for _ in 0..count {
            self.text.borrow_mut().pop();
        }
    }
    async fn apply_move_cursor(&self, byte_position: usize) {
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
    last_op: std::cell::RefCell<Option<String>>,
}

#[async_trait::async_trait(?Send)]
impl SutBlockTreeWrite for MiniBlockTreeSut {
    async fn apply_split_block(&self, id: &CapBlockId, position: usize) {
        *self.last_op.borrow_mut() = Some(format!("split {id} @ {position}"));
    }
    async fn apply_join_block(&self, id: &CapBlockId) {
        *self.last_op.borrow_mut() = Some(format!("join {id}"));
    }
    async fn apply_indent(&self, id: &CapBlockId) {
        *self.last_op.borrow_mut() = Some(format!("indent {id}"));
    }
    async fn apply_outdent(&self, id: &CapBlockId) {
        *self.last_op.borrow_mut() = Some(format!("outdent {id}"));
    }
    async fn apply_move_up(&self, id: &CapBlockId) {
        *self.last_op.borrow_mut() = Some(format!("move_up {id}"));
    }
    async fn apply_move_down(&self, id: &CapBlockId) {
        *self.last_op.borrow_mut() = Some(format!("move_down {id}"));
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
