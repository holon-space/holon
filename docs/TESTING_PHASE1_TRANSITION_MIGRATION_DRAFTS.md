# Phase 1 P1.3 — Two-transition migration drafts (paper spike)

**Status**: paper drafts. Not compiled this session — produces the diff shape that the next session applies + compiles. Validates H1, H2, H10 at the design level.

**Method**: rewrite `type_chars.rs` (narrow) and `split_block.rs` (structural) against `holon_pbt_core::capabilities` traits + `holon_pbt_core::TransitionImpl<R, S>` (H12 Option B). Compare to current files.

---

## Migration 1 — `type_chars.rs`

### Imports (changes)

```diff
-use crate::pbt::validation::{Reason, check};
-use holon_api::Region;
-use proptest::prelude::*;
-use proptest::strategy::BoxedStrategy;
-use validated::Validated;
-
-use super::E2ETransitionImpl;
-use crate::pbt::reference_state::ReferenceState;
-use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
+use crate::pbt::validation::{Reason, check};
+use proptest::prelude::*;
+use proptest::strategy::BoxedStrategy;
+use validated::Validated;
+
+use holon_pbt_core::capabilities::{
+    CapRegion, RefEditorMirror, RefEditorMirrorMut, RefFocus, RefLifecycle,
+    RefBlockTreeMut, SutEditorMirrorWrite, commit_active_editor_if_changed,
+};
+use holon_pbt_core::{TransitionFactory, TransitionImpl};
```

### `E2ETransitionFactory` → `TransitionFactory<R>`

```diff
-impl E2ETransitionFactory for TypeChars {
-    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
+impl<R> TransitionFactory<R> for TypeChars
+where R: RefEditorMirror + RefFocus + RefLifecycle,
+{
+    type Reason = Reason;
+    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
         let probe = TypeChars { text: String::new() };
-        probe.preconditions(state).map(|_| {
-            let last = state.last_transition_kind;
+        // preconditions needs both Mut and read — but factory only reads,
+        // so split: probe-via-helper or duplicate preconditions logic.
+        // Easiest: keep `preconditions` callable on probe via a separate
+        // free fn; factory uses it.
+        Self::preconditions_static(state).map(|_| {
+            let last = state.last_transition_kind();
             let tc_weight = match last { ... };
             ...
         })
     }
 }
```

**Surface issue**: `preconditions` lives on `TransitionImpl` and needs `&mut R`-equivalent constraints (for symmetry with apply_to_ref). The factory only has `&R`. Today's code calls `probe.preconditions(state)` and `preconditions` works on `&ReferenceState` — clean. Under capabilities, we want `preconditions: &R -> Validated`, and apply_to_ref takes `&mut R`. So **split `preconditions` into a static-ish form**:

```rust
impl TypeChars {
    fn preconditions_static<R: RefEditorMirror + RefFocus + RefLifecycle>(state: &R) -> Validated<(), Reason> {
        let checks = vec![
            check(R::atomic_editor_enabled(), Reason::AtomicEditorDisabled),
            check(state.enable_loro(), Reason::LoroRequiredForAtomicEditor),
            check(state.app_started(), Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
            check(state.current_focus(CapRegion::Main).is_some(), Reason::NoFocusInMain),
            check(state.active_editor_block().is_some(), Reason::NoActiveEditor),
        ];
        checks.into_iter().collect::<Validated<Vec<()>, _>>().map(|_| ())
    }
}
```

Then `TransitionImpl::preconditions(&self, state)` delegates: `Self::preconditions_static(state)`. Or move `preconditions` to a free function. **Recommend free function** — instance has no relevant data for the check.

### `apply_to_ref` (capability-bound)

```diff
-    fn apply_to_ref(&self, state: &mut ReferenceState) {
-        if let Some(editor) = state.active_editor.as_mut() {
-            editor.type_chars(&self.text);
-        }
-        if state.variant.enable_loro {
-            state.commit_active_editor_if_changed();
-        }
-    }
+    fn apply_to_ref(&self, state: &mut R) where R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle {
+        state.type_chars(&self.text);
+        if state.enable_loro() {
+            commit_active_editor_if_changed(state);
+        }
+    }
```

### `apply_to_sut` (capability-bound)

```diff
-    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
-        sut.apply_type_chars(&self.text).await;
-    }
+    async fn apply_to_sut<S: SutEditorMirrorWrite + ?Sized>(&self, _: &R, sut: &mut S) {
+        sut.apply_type_chars(&self.text).await;
+    }
```

### Full impl shape

```rust
impl<R, S> TransitionImpl<R, S> for TypeChars
where
    R: RefEditorMirror + RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle,
    S: SutEditorMirrorWrite + ?Sized,
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        Self::preconditions_static(state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.type_chars(&self.text);
        if state.enable_loro() {
            commit_active_editor_if_changed(state);
        }
    }

    async fn apply_to_sut(&self, _: &R, sut: &mut S) {
        sut.apply_type_chars(&self.text).await;
    }
}
```

### LOC diff estimate

Original `type_chars.rs`: 110 LOC.
Migrated: ~115 LOC (adds `where` clauses, helper static method, drops a few imports).
**Net diff: +5 LOC. H10: PASS (well under 50).**

---

## Migration 2 — `split_block.rs`

### Imports

```diff
-use holon_api::ContentType;
-use holon_api::Region;
-use holon_api::entity_uri::EntityUri;
-use proptest::prelude::*;
-use proptest::strategy::BoxedStrategy;
-use validated::Validated;
-
-use super::E2ETransitionImpl;
-use crate::pbt::reference_state::{CursorPosition, ReferenceState};
-use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
-use crate::pbt::validation::{Reason, check};
+use proptest::prelude::*;
+use proptest::strategy::BoxedStrategy;
+use validated::Validated;
+
+use holon_pbt_core::capabilities::{
+    CapBlockId, CapCursor, CapRegion,
+    RefBlockTree, RefBlockTreeMut, RefFocus, RefFocusMut, RefLifecycle,
+    SutBlockTreeWrite,
+};
+use holon_pbt_core::{TransitionFactory, TransitionImpl};
+use crate::pbt::validation::{Reason, check};
```

### Field type change

```diff
 pub struct SplitBlock {
-    pub block_id: EntityUri,
+    pub block_id: CapBlockId,   // String — wide PBT translates via .to_string() / .parse()
     pub position: usize,
 }
```

**Friction point**: `EntityUri` is structured; `CapBlockId = String` is stringly-typed. The wide PBT's existing `apply_split_block` takes `&holon_api::EntityUri`. Two options:

- (A) Migrate `block_id` to `CapBlockId = String`, wide PBT parses back to `EntityUri` at SUT boundary. Loses the type-driven safety locally.
- (B) Keep `EntityUri` as the wide PBT's block-id type, parameterize the capability trait with an associated type `type Id`.

Option B is type-purer; Option A is simpler. **Recommend Option A for Stage A** — minimal change; can promote to Option B later if friction grows. Cost: one `.to_string()` at the EntityUri boundary.

Updating `capabilities.rs`'s `CapBlockId` doc to call this out.

### Factory

```rust
impl<R> TransitionFactory<R> for SplitBlock
where R: RefBlockTree + RefLifecycle,
{
    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates: Vec<(CapBlockId, usize)> = {
            let editable = state.main_editable_descendants();
            let mut result = vec![];
            for id in editable {
                if let Some(text) = state.block_content(&id) {
                    let content_len = text.len();
                    for position in 0..=content_len {
                        let probe = SplitBlock { block_id: id.clone(), position };
                        if probe.preconditions(state).is_good() {
                            result.push((id.clone(), position));
                        }
                    }
                }
            }
            result
        };
        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|(block_id, position)| SplitBlock { block_id, position })
                .boxed();
            (100, strat)
        })
    }
}
```

Note: `state.block_content(&id).map(|s| s.len())` replaces `state.block_state.blocks.get(&id).map(|b| b.content_text().len())`. Same info, narrower API.

### preconditions

```rust
fn preconditions(&self, state: &R) -> Validated<(), Reason>
where R: RefBlockTree + RefLifecycle,
{
    let focus_roots = state.focus_root_ids(CapRegion::Main);
    let mut checks = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(state.is_properly_setup(), Reason::NotProperlySetup),
    ];

    checks.push(check(state.is_text_block(&self.block_id), Reason::FocusedNotText));
    if let Some(text) = state.block_content(&self.block_id) {
        checks.push(check(self.position <= text.len(), Reason::PreconditionFailed));
    } else {
        checks.push(check(false, Reason::FocusedBlockMissing));
    }
    checks.push(check(!state.is_layout_block(&self.block_id), Reason::FocusedInLayoutBlocks));
    checks.push(check(
        state.is_descendant_of_any(&self.block_id, &focus_roots),
        Reason::FocusedNotDescendantOfFocusRoot,
    ));
    checks.into_iter().collect::<Validated<Vec<()>, _>>().map(|_| ())
}
```

### apply_to_ref

```rust
fn apply_to_ref(&self, state: &mut R)
where R: RefBlockTreeMut + RefFocusMut,
{
    state.push_undo_snapshot();
    let new_id = state.split_block(&self.block_id, self.position);
    state.set_focus(CapRegion::Main, new_id, CapCursor::default());
}
```

**Discovery — H2 cross-cut #2 confirmed**: the "set focus to newly-created block" pattern is inline; same shape in Join, Indent, Outdent. Worth extracting as `refocus_after_create<R: RefFocusMut>(state, new_id, region)` if it grows past 4 callers — but inline 3 lines is fine for now.

### apply_to_sut

```rust
async fn apply_to_sut(&self, _: &R, sut: &mut S) where S: SutBlockTreeWrite + ?Sized {
    sut.apply_split_block(&self.block_id, self.position).await;
}
```

**Note**: `apply_split_block` lost its `ref_state` parameter (per Phase 1 mapping recommendation). The wide-PBT `E2ESut` impl keeps the mapping internal.

### LOC diff estimate

Original `split_block.rs`: 151 LOC.
Migrated: ~155 LOC.
**Net diff: +4 LOC. H10: PASS (well under 150 worst-case).**

---

## Cross-cut enumeration (H2 final, for the seven T0 transitions)

1. **`commit_active_editor_if_changed`** (TypeChars, DeleteBackward when `enable_loro`) — `RefEditorMirrorMut + RefBlockTreeMut + RefFocus`. Lifted to free function in `capabilities.rs`. ✅
2. **Focus follow-up on tree mutation** (Split, Join, Indent, Outdent) — `RefFocusMut`. Pattern: after mutation, `set_focus(region, new_id, cursor)`. Inlined per transition (3-4 lines). Recommendation: leave inline; promote to helper if pattern grows.
3. **Probe-based factory precondition** (all seven) — factory calls `preconditions` via a probe instance. With the trait split, the factory bound (`&R`) is narrower than preconditions' bound. Mitigation: free static helper `preconditions_static(state)` per transition; instance method delegates. Mechanical, ~5 LOC per file.

Total cross-cuts: **3** (one free function, one inline-with-pattern, one mechanical-helper). Under the "if cross-cuts grow past ~6 free functions" limit from the plan.

---

## Per-transition LOC budget (final estimate)

| Transition | Current LOC | Migrated LOC | Diff |
|---|---:|---:|---:|
| `type_chars.rs` | 110 | ~115 | +5 |
| `delete_backward.rs` | 118 | ~123 | +5 |
| `move_cursor.rs` | 100 | ~105 | +5 |
| `move_up.rs` | 122 | ~127 | +5 |
| `move_down.rs` | 115 | ~120 | +5 |
| `split_block.rs` | 151 | ~155 | +4 |
| `join_block.rs` | 173 | ~178 | +5 |
| `indent.rs` | 121 | ~126 | +5 |
| `outdent.rs` | 111 | ~116 | +5 |

Mean diff: **+5 LOC**. Max: **+5 LOC**. H10 budget was median <50, worst <150. **PASS with enormous margin** — the migration is genuinely mechanical.

The +5 LOC per file is structural overhead: `where` clauses, `preconditions_static` helper, `TransitionImpl<R, S>` impl head. No new logic; just shape changes.

---

## Open friction for next-session compile

1. **`CapBlockId = String` vs `EntityUri`**: trait surface uses String; wide PBT's existing methods take `EntityUri`. The blanket impls on `ReferenceState` need to bridge:
   ```rust
   impl RefBlockTreeMut for ReferenceState {
       fn split_block(&mut self, id: &CapBlockId, position: usize) -> CapBlockId {
           let uri = EntityUri::parse(id).expect("CapBlockId must parse as EntityUri in wide PBT");
           self.split_block(&uri, position).to_string()
       }
       // ...
   }
   ```
   The `.expect()` is fine — wide PBT only ever generates valid EntityUris. Document at the impl site.

2. **`probe.preconditions(state)` factory pattern needs the `preconditions_static` helper.** Mechanical; ~5 LOC per file.

3. **`async fn apply_to_sut<S: ... + ?Sized>(&self, _: &R, sut: &mut S)`** — `?Sized` so wide PBT can keep using `&mut dyn SutHandle` (which becomes `&mut dyn SutBlockTreeWrite + SutEditorMirrorWrite + SutFocusWrite + SutQuiesce` — a wide trait object). Cleanest with bundled umbrella trait `SutTransitionTarget` (PbtSlicing.md §6.5):
   ```rust
   pub trait SutTransitionTarget: SutBlockTreeWrite + SutEditorMirrorWrite + SutFocusWrite + SutQuiesce {}
   impl<T: ?Sized + SutBlockTreeWrite + SutEditorMirrorWrite + SutFocusWrite + SutQuiesce> SutTransitionTarget for T {}
   ```

## Suggested patch application order for next session

1. Apply the capability trait additions (`move_block`, `swap_siblings`, `set_block_content`, `SutTransitionTarget` umbrella) — already in `capabilities.rs`.
2. Write blanket impls on `ReferenceState` (Phase 2 work — but a single transition needs at minimum the impls of its caps).
3. Migrate `type_chars.rs` end-to-end. Compile.
4. Migrate `split_block.rs` end-to-end. Compile.
5. Run the wide PBT (compile-only check + smoke test on one seed) to confirm no regression.
6. Repeat for the remaining 5 transitions.
7. Then write `EditorPureRef` / `EditorPureSut` and `editor_pure_pbt.rs` — at this point Phase 5 is ~100-200 LOC of new code.
