//! Birth-on-focus for a collection's creation affordance.
//!
//! A creation affordance is a rendered row, never a block: it carries no
//! content, mounts no editor, and can hold no caret. The instant focus reaches
//! it a real (empty) block is born under the resolved parent and the caret
//! seats in that block, so every downstream gesture — typing, indent, Enter,
//! breadcrumb — acts on a real block. That is what makes a pre-persist block
//! state unnecessary.
//!
//! A block born this way and never given content is *ephemeral*: the reaper
//! deletes it once focus genuinely leaves. The first non-empty content retires
//! the id from [`EphemeralNewborns`] **permanently**, so a block the user typed
//! into and then cleared survives — deliberate emptiness is user intent.

use std::collections::HashSet;
use std::sync::Mutex;

use holon_api::EntityUri;

/// Provenance stamp for the birth and the reap. Both are system firings, not
/// human gestures, so they carry [`holon_api::OpOrigin::Rule`] and never enter
/// the user undo stack.
pub const BIRTH_TRANSITION_ID: &str = "creation-slot-birth";
pub const REAP_TRANSITION_ID: &str = "creation-slot-reap";

/// The ids of empty blocks born by a creation affordance that have never
/// carried content.
///
/// Membership is the *only* licence to reap. Entry happens at birth; exit
/// happens on the first non-empty content and is irreversible — re-clearing a
/// block does not make it reapable again.
#[derive(Default)]
struct Registry {
    ids: HashSet<EntityUri>,
    /// affordance id → the still-empty block its last birth produced. This is
    /// what makes a birth idempotent: focus landing on the same affordance
    /// again re-enters the block already born there instead of minting a
    /// second one. The entry is dropped the moment that block is typed into or
    /// reaped, so the NEXT focus on the affordance is a genuine new birth.
    born_at: std::collections::HashMap<String, EntityUri>,
}

#[derive(Default)]
pub struct EphemeralNewborns {
    inner: Mutex<Registry>,
}

impl EphemeralNewborns {
    pub fn new() -> Self {
        Self::default()
    }

    /// The still-empty block a previous focus on `affordance` already brought
    /// into existence. `Some` means: do not mint a second one, re-enter this.
    pub fn already_born_at(&self, affordance: &str) -> Option<EntityUri> {
        self.inner.lock().unwrap().born_at.get(affordance).cloned()
    }

    /// Record a freshly born, still-empty block and the affordance it came
    /// from.
    pub fn record(&self, affordance: &str, id: EntityUri) {
        let mut inner = self.inner.lock().unwrap();
        inner.ids.insert(id.clone());
        inner.born_at.insert(affordance.to_string(), id);
    }

    /// Retire `id` — it has carried content and is the user's block now.
    /// Returns whether it had been ephemeral. Idempotent.
    pub fn retire(&self, id: &EntityUri) -> bool {
        let mut inner = self.inner.lock().unwrap();
        inner.born_at.retain(|_, born| born != id);
        inner.ids.remove(id)
    }

    pub fn is_ephemeral(&self, id: &EntityUri) -> bool {
        self.inner.lock().unwrap().ids.contains(id)
    }

    /// Every ephemeral newborn except `keep` (the block focus just moved to).
    /// Removing them from the set here makes the reap single-shot: a second
    /// focus move cannot re-dispatch a delete for the same id.
    pub fn take_all_except(&self, keep: Option<&EntityUri>) -> Vec<EntityUri> {
        let mut inner = self.inner.lock().unwrap();
        let taken: Vec<EntityUri> = inner
            .ids
            .iter()
            .filter(|id| Some(*id) != keep)
            .cloned()
            .collect();
        for id in &taken {
            inner.ids.remove(id);
            inner.born_at.retain(|_, born| born != id);
        }
        taken
    }
}

/// Whether `content` read back from the store at reap time still licenses a
/// reap. The ephemeral set is the primary proof (retired synchronously by the
/// keystroke sink); this is the independent store-side one, so a write that
/// never passed through this frontend's editor — MCP, a peer sync, a rule —
/// also saves the block.
pub fn reap_is_licensed_by_store(content: &str) -> bool {
    content.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> EntityUri {
        EntityUri::parse(s).unwrap()
    }

    #[test]
    fn a_recorded_newborn_is_reapable_until_it_carries_content() {
        let set = EphemeralNewborns::new();
        let id = uri("block:n1");
        set.record("block:__virtual:p", id.clone());
        assert!(set.is_ephemeral(&id));
        assert!(set.retire(&id));
        assert!(!set.is_ephemeral(&id));
    }

    /// Care (1): a block the user typed into and then CLEARED must survive.
    /// Retirement is irreversible, so re-emptying never re-arms the reaper.
    #[test]
    fn a_typed_then_cleared_block_never_becomes_reapable_again() {
        let set = EphemeralNewborns::new();
        let id = uri("block:n1");
        set.record("block:__virtual:p", id.clone());
        set.retire(&id); // user typed
        // User selects all and deletes: the block is empty again, but it is
        // the user's block now.
        assert!(!set.is_ephemeral(&id));
        assert!(set.take_all_except(None).is_empty());
    }

    /// Care (2): the block focus moved TO is never reaped, even while empty —
    /// it is the block the caret is sitting in.
    #[test]
    fn the_newly_focused_newborn_is_not_reaped() {
        let set = EphemeralNewborns::new();
        let old = uri("block:old");
        let new = uri("block:new");
        set.record("block:__virtual:pa", old.clone());
        set.record("block:__virtual:pb", new.clone());
        let reaped = set.take_all_except(Some(&new));
        assert_eq!(reaped, vec![old]);
        assert!(set.is_ephemeral(&new));
    }

    /// Single-shot: a second focus move must not re-dispatch a delete for an
    /// id whose reap already went out.
    #[test]
    fn taking_a_newborn_removes_it_so_the_reap_cannot_repeat() {
        let set = EphemeralNewborns::new();
        set.record("block:__virtual:p", uri("block:n1"));
        assert_eq!(set.take_all_except(None).len(), 1);
        assert!(set.take_all_except(None).is_empty());
    }

    /// Birth is IDEMPOTENT per affordance. Two focus events on the same
    /// affordance — a re-click, a focus restore, a re-render that re-seats
    /// focus — must produce ONE block: the second re-enters the block the first
    /// brought into existence. Without this the keystone caught a second empty
    /// block per extra focus event, and a live vault would accumulate one per
    /// stray click.
    #[test]
    fn two_focus_events_on_one_affordance_birth_exactly_one_block() {
        let set = EphemeralNewborns::new();
        let affordance = "block:__virtual:page-1";

        // Focus event 1: nothing born here yet, so the engine mints.
        assert_eq!(set.already_born_at(affordance), None);
        let first = uri("block:newborn-1");
        set.record(affordance, first.clone());

        // Focus event 2: the engine re-enters the SAME block, mints nothing.
        assert_eq!(
            set.already_born_at(affordance),
            Some(first),
            "a second focus on the same affordance must re-enter the block already born there"
        );
    }

    /// ...and once that block is the user's (typed into) or gone (reaped), the
    /// affordance is free again, so the NEXT focus is a genuine new birth.
    #[test]
    fn an_affordance_births_again_once_its_newborn_is_typed_into_or_reaped() {
        let affordance = "block:__virtual:page-1";

        let typed = EphemeralNewborns::new();
        let a = uri("block:newborn-1");
        typed.record(affordance, a.clone());
        typed.retire(&a);
        assert_eq!(
            typed.already_born_at(affordance),
            None,
            "after the user types, the affordance must birth a NEW block, not re-enter theirs"
        );

        let reaped = EphemeralNewborns::new();
        let b = uri("block:newborn-2");
        reaped.record(affordance, b);
        reaped.take_all_except(None);
        assert_eq!(
            reaped.already_born_at(affordance),
            None,
            "after the reap, the affordance must not re-enter a block that no longer exists"
        );
    }

    #[test]
    fn a_store_read_of_non_empty_content_refuses_the_reap() {
        assert!(reap_is_licensed_by_store(""));
        assert!(!reap_is_licensed_by_store("typed by someone else"));
    }
}
