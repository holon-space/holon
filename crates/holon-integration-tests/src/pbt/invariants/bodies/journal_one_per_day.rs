//! `inv-journal-one-per-day` (ADR 0024 §6 daily-journal rule).
//!
//! @pbt oracle correspondence
//! @pbt covers journal-per-day — the production `journals_auto_create` rule
//!   must produce EXACTLY ONE journal date-page per calendar day the clock has
//!   visited (boot + each `AdvanceDay` rollover), and none for any other day.
//! @pbt slips-if-removed a day-rollover CDC that fires the journal action twice
//!   (a second block for the same date) or not at all (a visited day with no
//!   journal), or a rollover that mints a journal for the WRONG day — the
//!   block-comparison invariants would still pass if the ref modelled the same
//!   wrong shape, but this clock-anchored oracle pins the rule's output to the
//!   reference clock's visited-day SET independently.
//!
//! ## Identity, not parentage
//!
//! A journal date-page is identified by its CANONICAL page identity —
//! `PageId::for_path("Journals/{date}")` — NOT by a `parent_id = block:journals`
//! scan. The rule emits `place: page(journals)`, so a fired journal is a
//! self-documenting doc-root that MATERIALISES its own `Journals/{date}.org`
//! file; in the SUT's `block_raw` its `parent_id` is therefore the document-root
//! sentinel, not `block:journals` (the block-comparison invariants only agree
//! because `normalize_block` reparents both sides' doc-roots to
//! `__document_root__`). A parent scan misses it; the deterministic id does not.
//!
//! Asserts every date whose canonical journal page exists in the SUT is a day
//! the reference clock actually VISITED (`SUT journal dates ⊆ visited_days`) —
//! i.e. the rule never mints a journal for a wrong/unvisited day. Completeness
//! (every visited day HAS a journal) is intentionally left to the
//! block-comparison invariants (`SUT journals == reference journals`), because
//! whether the rule fires at all is wiring-dependent while `visited_days` is
//! not — asserting it here would false-RED an otherwise-valid dormant draw.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::RefClock;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvJournalOnePerDay;

impl InvJournalOnePerDay {
    pub const ID: InvariantId = InvariantId("inv-journal-one-per-day");

    /// Is `id` the canonical journal page for calendar date `date`
    /// (`PageId::for_path("Journals/{date}")`)? The rule's page branch mints
    /// exactly this id, so it identifies a journal date-page regardless of the
    /// projection's stored `parent_id`.
    fn is_canonical_journal(id: &holon_pbt_core::capabilities::EntityUri, date: &str) -> bool {
        holon_api::link_parser::PageId::for_path(&format!("Journals/{date}"))
            .map(|p| p.into_entity_uri() == *id)
            .unwrap_or(false)
    }
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvJournalOnePerDay
where
    R: RefClock,
    S: SutSqlProjection,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // The SUT's journal date-pages, by canonical identity: a block whose
        // content parses as a calendar date AND whose id is the canonical
        // journal id for that date. A `BTreeSet` folds any accidental duplicate
        // (which cannot happen for a canonical id anyway — the rule upserts).
        let mut sut_dates: BTreeSet<String> = BTreeSet::new();
        for id in sut.all_block_ids().await {
            let Some(content) = sut.block_content(&id).await else {
                continue;
            };
            let Ok(parsed) = holon_api::CalendarDate::parse(&content) else {
                continue;
            };
            let day = parsed.ymd();
            if Self::is_canonical_journal(&id, &day) {
                sut_dates.insert(day);
            }
        }

        // Clock anchor, SAFE direction: every journal the rule PRODUCED must be
        // for a day the clock actually visited (boot + each rollover). A journal
        // for an unvisited day is a real defect — a rollover minting the WRONG
        // date, or a spurious extra fire. The CONVERSE (every visited day has a
        // journal) is deliberately NOT asserted here: whether the rule fires is
        // wiring-dependent (a `ViewModel`+Turso draw whose action pipeline is
        // dormant produces none, yet `visited_days` still holds the boot day),
        // and the block-comparison invariants already pin "SUT journals ==
        // reference journals" for the wirings that DO fire — so completeness is
        // covered there, and folding it in here would false-RED the dormant
        // wirings. Canonical ids make "one per day" structural (the rule upserts
        // the same id), so no separate duplicate check is needed.
        let visited = ref_.visited_days();
        let spurious: Vec<&String> = sut_dates.difference(&visited).collect();
        if !spurious.is_empty() {
            return InvariantResult::Fail(format!(
                "inv-journal-one-per-day: SUT has journal date-page(s) for {spurious:?} the \
                 clock never visited (visited days {visited:?}, SUT journal dates {sut_dates:?}) \
                 — a day-rollover minted a journal for the wrong/unvisited date"
            ));
        }

        InvariantResult::Ok
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use holon_pbt_core::capabilities::EntityUri;
    use holon_pbt_core::capabilities::RefClock;
    use holon_pbt_core::capabilities::SutSqlProjection;
    use holon_pbt_core::invariant::Invariant;
    use holon_pbt_core::invariant::InvariantResult;

    use super::InvJournalOnePerDay;

    /// Reference clock stub: fixed `visited_days`.
    struct ClockStub {
        visited: BTreeSet<String>,
    }
    impl RefClock for ClockStub {
        fn today(&self) -> String {
            self.visited.iter().next_back().cloned().unwrap_or_default()
        }
        fn expected_journal_day_count(&self) -> usize {
            self.visited.len()
        }
        fn visited_days(&self) -> BTreeSet<String> {
            self.visited.clone()
        }
    }

    /// SUT projection stub: hosts a fixed set of `(canonical-journal-id, date)`
    /// blocks; every other read is empty/None (unused by this invariant).
    struct SqlStub {
        journals: Vec<(EntityUri, String)>,
    }
    impl SqlStub {
        fn for_dates(dates: &[&str]) -> Self {
            let journals = dates
                .iter()
                .map(|d| {
                    let id = holon_api::link_parser::PageId::for_path(&format!("Journals/{d}"))
                        .unwrap()
                        .into_entity_uri();
                    (id, (*d).to_string())
                })
                .collect();
            Self { journals }
        }
    }
    #[async_trait::async_trait(?Send)]
    impl SutSqlProjection for SqlStub {
        async fn block_row(&self, _: &EntityUri) -> Option<Vec<String>> {
            None
        }
        async fn all_block_ids(&self) -> BTreeSet<EntityUri> {
            self.journals.iter().map(|(id, _)| id.clone()).collect()
        }
        async fn sorted_children(&self, _: &EntityUri) -> Vec<EntityUri> {
            Vec::new()
        }
        async fn watch_row_count(&self, _: &str) -> Option<usize> {
            None
        }
        async fn block_raw_row(&self, _: &EntityUri) -> Option<Vec<String>> {
            None
        }
        async fn block_tag_block_ids(&self) -> BTreeSet<EntityUri> {
            BTreeSet::new()
        }
        async fn block_task_state(&self, _: &EntityUri) -> Option<String> {
            None
        }
        async fn block_content(&self, id: &EntityUri) -> Option<String> {
            self.journals
                .iter()
                .find(|(jid, _)| jid == id)
                .map(|(_, d)| d.clone())
        }
    }

    fn visited(days: &[&str]) -> BTreeSet<String> {
        days.iter().map(|d| d.to_string()).collect()
    }

    /// Positive: every SUT journal is a visited day ⇒ Ok. Fewer SUT journals
    /// than visited is allowed (completeness is the block-comparison arms').
    #[tokio::test]
    async fn ok_when_every_sut_journal_is_a_visited_day() {
        let clock = ClockStub {
            visited: visited(&["2026-01-15", "2026-01-16", "2026-01-17"]),
        };
        // SUT fired only two of the three visited days — still Ok (subset).
        let sut = SqlStub::for_dates(&["2026-01-15", "2026-01-16"]);
        assert!(matches!(
            InvJournalOnePerDay.check(&clock, &sut).await,
            InvariantResult::Ok
        ));
    }

    /// MUTATION-CHECK: perturb the reference visited-day set so it no longer
    /// contains a day the SUT has a journal for (drop `2026-01-16`). The SUT's
    /// journal for that now-unvisited day is spurious ⇒ the invariant MUST Fail.
    /// Proves the oracle is non-vacuous: it detects a journal minted for a day
    /// the clock never visited (a wrong-date rollover).
    #[tokio::test]
    async fn fail_when_sut_journal_for_unvisited_day() {
        let clock = ClockStub {
            // Reference clock visited only boot — NOT 2026-01-16.
            visited: visited(&["2026-01-15"]),
        };
        // SUT holds a journal for 2026-01-16 (unvisited) — the mutation.
        let sut = SqlStub::for_dates(&["2026-01-15", "2026-01-16"]);
        match InvJournalOnePerDay.check(&clock, &sut).await {
            InvariantResult::Fail(msg) => {
                assert!(msg.contains("2026-01-16"), "must name the spurious day: {msg}");
                assert!(msg.contains("never visited"), "must explain the defect: {msg}");
            }
            other => panic!("mutation must RED, got {other:?}"),
        }
    }
}
