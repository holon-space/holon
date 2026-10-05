//! A subscriber's replay comes out in the order the conditions were RAISED.
//!
//! The bus holds its conditions in a `LiveData` keyed subject-first, so the
//! holder's own order is alphabetical by subject. Replay order is what a reader
//! sees as toast stack order: replaying alphabetically would reshuffle the
//! stack by subject name whenever a window opens, which is not the order
//! anything happened in.
//!
//! Both cases raise conditions whose subjects sort the OPPOSITE way to the
//! order they were raised in, so an implementation that replays the holder's
//! key order fails them and one that happens to be insertion-ordered passes.
//!
//! @pbt kind unit
//! @pbt covers condition-bus-replay-raise-order
//! @pbt slips-if-removed the toast stack silently reorders itself by subject
//! name on every window open, while every keyed-upsert assertion stays green

use holon_api::condition_bus::Condition;
use holon_api::condition_bus::ConditionBus;
use holon_api::condition_bus::ConditionKind;
use holon_api::condition_bus::RefusedFile;

fn emptied_file(subject: &str) -> Condition {
    Condition {
        subject: subject.to_string(),
        reason: ConditionKind::VaultFileEmptied,
    }
}

fn subjects(bus: &ConditionBus) -> Vec<String> {
    bus.subscribe()
        .current
        .into_iter()
        .map(|c| c.subject)
        .collect()
}

#[test]
fn replay_follows_raise_order_not_subject_order() {
    let bus = ConditionBus::new();
    // "zeta" first, "alpha" second: raise order and alphabetical order disagree.
    bus.emit(emptied_file("zeta.org"));
    bus.emit(emptied_file("alpha.org"));
    bus.emit(emptied_file("mid.org"));

    assert_eq!(
        subjects(&bus),
        vec![
            "zeta.org".to_string(),
            "alpha.org".to_string(),
            "mid.org".to_string()
        ],
        "replay must follow the order the conditions were raised in"
    );
}

#[test]
fn re_raising_keeps_a_conditions_original_position() {
    let bus = ConditionBus::new();
    bus.emit(emptied_file("zeta.org"));
    bus.emit(emptied_file("alpha.org"));
    // The same failure again: it upserts, and must NOT jump to the end. A
    // repeated failure that reorders the stack makes the other toasts move
    // under the reader's eyes for no event they can see.
    bus.emit(emptied_file("zeta.org"));

    assert_eq!(
        subjects(&bus),
        vec!["zeta.org".to_string(), "alpha.org".to_string()],
        "a re-raise upserts in place; it does not move the condition"
    );
}

#[test]
fn a_cleared_condition_that_returns_takes_the_newest_position() {
    let bus = ConditionBus::new();
    let first = emptied_file("zeta.org");
    bus.emit(first.clone());
    bus.emit(emptied_file("alpha.org"));
    bus.clear(&first.condition_key());
    // It really did end and really did happen again, so it is the newest
    // thing the reader has to look at — unlike a re-raise of one still in
    // effect.
    bus.emit(emptied_file("zeta.org"));

    assert_eq!(
        subjects(&bus),
        vec!["alpha.org".to_string(), "zeta.org".to_string()],
        "a condition that was cleared and raised again is the newest one"
    );
}

/// A format's refusals are emitted by the bus itself, while it holds its own
/// record of refused files, rather than by a caller's `emit`.
#[test]
fn grouped_refusals_replay_in_the_order_their_formats_were_first_refused() {
    let bus = ConditionBus::new();
    let refused = |path: &str| RefusedFile {
        path: path.to_string(),
        reason: "unparseable".to_string(),
    };
    bus.vault_ingest_refused("org", refused("b.org"));
    bus.vault_ingest_refused("cooklang", refused("a.cook"));
    // A second file of the first format re-raises its condition in place.
    bus.vault_ingest_refused("org", refused("c.org"));
    bus.emit(emptied_file("aaa.org"));

    assert_eq!(
        subjects(&bus),
        vec![
            "org".to_string(),
            "cooklang".to_string(),
            "aaa.org".to_string()
        ],
        "a late subscriber replays grouped refusals in the order they were raised"
    );
}
