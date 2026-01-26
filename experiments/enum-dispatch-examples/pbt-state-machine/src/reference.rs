//! Reference state — the simple model proptest-state-machine compares against.
//!
//! A list of named items addressed by stable u32 ids. Newly added items
//! get the next free id. The SUT is expected to converge to the same
//! `(id, name)` set after every transition.

use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct RefState {
    pub items: BTreeMap<u32, String>,
    pub next_id: u32,
}

impl RefState {
    pub fn add(&mut self, name: String) -> u32 {
        let id = self.next_id;
        self.items.insert(id, name);
        self.next_id += 1;
        id
    }

    pub fn remove(&mut self, id: u32) {
        self.items.remove(&id);
    }

    pub fn rename(&mut self, id: u32, new_name: String) {
        let slot = self.items.get_mut(&id).expect("rename of nonexistent id");
        *slot = new_name;
    }

    pub fn ids(&self) -> Vec<u32> {
        self.items.keys().copied().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
