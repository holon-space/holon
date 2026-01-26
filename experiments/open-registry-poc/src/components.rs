//! Three REAL components (§8.7 axes are real components, never fixtures):
//! - `BlockStore` (always-on) → `SutBlockTreeWrite` + `SutBlockRead`
//! - `ToggleStore` (optional axis `Toggle`) → `SutToggleWrite` +
//!   `SutToggleRead`
//! - `EditorComponent` (optional axis `Editor`) → `SutEditorWrite` +
//!   `SutEditorRead`
//!
//! Each holds its own state behind a `Mutex`; the one `Arc` is registered under
//! both of its caps and write caps mutate via interior mutability (§4.4).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;

use crate::core::CapMap;
use crate::core::CapProvider;
use crate::core::Subsystem;
use crate::core::SutBlockRead;
use crate::core::SutBlockTreeWrite;
use crate::core::SutEditorRead;
use crate::core::SutEditorWrite;
use crate::core::SutToggleRead;
use crate::core::SutToggleWrite;

// ── Always-on: block-tree store ───────────────────────────────────────────
pub struct BlockStore {
    blocks: Mutex<Vec<u64>>,
}
impl BlockStore {
    pub fn seeded(ids: &[u64]) -> Arc<Self> {
        Arc::new(BlockStore {
            blocks: Mutex::new(ids.to_vec()),
        })
    }
}
impl SutBlockTreeWrite for BlockStore {
    fn split(&self, target: u64, new_id: u64) {
        let mut b = self.blocks.lock().unwrap();
        let pos = b.iter().position(|x| *x == target).unwrap();
        b.insert(pos + 1, new_id);
    }
}
impl SutBlockRead for BlockStore {
    fn blocks(&self) -> Vec<u64> {
        self.blocks.lock().unwrap().clone()
    }
}
impl CapProvider for BlockStore {
    fn register(self: Arc<Self>, map: &mut CapMap) {
        map.insert::<dyn SutBlockTreeWrite>(self.clone());
        map.insert::<dyn SutBlockRead>(self.clone());
    }
}

// ── Optional axis: Toggle ─────────────────────────────────────────────────
#[derive(Default)]
pub struct ToggleStore {
    toggled: Mutex<BTreeSet<u64>>,
}
impl ToggleStore {
    pub fn new() -> Arc<Self> {
        Arc::new(ToggleStore::default())
    }
}
impl SutToggleWrite for ToggleStore {
    fn toggle(&self, target: u64) {
        let mut t = self.toggled.lock().unwrap();
        if !t.insert(target) {
            t.remove(&target);
        }
    }
}
impl SutToggleRead for ToggleStore {
    fn is_toggled(&self, id: u64) -> bool {
        self.toggled.lock().unwrap().contains(&id)
    }
}
impl CapProvider for ToggleStore {
    fn register(self: Arc<Self>, map: &mut CapMap) {
        map.insert::<dyn SutToggleWrite>(self.clone());
        map.insert::<dyn SutToggleRead>(self.clone());
    }
}

// ── Optional axis: Editor ─────────────────────────────────────────────────
#[derive(Default)]
pub struct EditorComponent {
    text: Mutex<String>,
}
impl EditorComponent {
    pub fn new() -> Arc<Self> {
        Arc::new(EditorComponent::default())
    }
}
impl SutEditorWrite for EditorComponent {
    fn type_char(&self, ch: char) {
        self.text.lock().unwrap().push(ch);
    }
}
impl SutEditorRead for EditorComponent {
    fn text(&self) -> String {
        self.text.lock().unwrap().clone()
    }
}
impl CapProvider for EditorComponent {
    fn register(self: Arc<Self>, map: &mut CapMap) {
        map.insert::<dyn SutEditorWrite>(self.clone());
        map.insert::<dyn SutEditorRead>(self.clone());
    }
}

// ── Config-driven SUT assembly — "a slice is the component list" (§6/§8.7) ──
/// Build the composed SUT for an active optional-subsystem set. BlockTree is
/// always on; each present `Subsystem` adds its real component.
pub fn build_sut(active: &[Subsystem], seed: &[u64]) -> CapMap {
    let mut map = CapMap::new().with_arc(BlockStore::seeded(seed));
    if active.contains(&Subsystem::Toggle) {
        map = map.with_arc(ToggleStore::new());
    }
    if active.contains(&Subsystem::Editor) {
        map = map.with_arc(EditorComponent::new());
    }
    map
}
