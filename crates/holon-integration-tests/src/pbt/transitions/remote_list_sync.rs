//! Transition: change the list — on the fixture peer, or by authoring a row in
//! Holon — and run ONE remote-list sync round, then run a second round to
//! prove the converged round is a no-op.
//!
//! @pbt rung remote-list
//! @pbt covers remote-list-mirror-matches-ref — the round drives the REAL
//!   [`holon_connections::sync_once`] over a fixture peer and the real mirror
//!   table; the invariant then asserts the mirror equals the peer's list.
//!
//! The peer is the oracle. The SUT half mutates the peer the same way the ref
//! half does, runs the production round (pull → reconcile → apply local
//! intents through the dispatcher → push), and then runs a SECOND round over
//! the unchanged peer and fails loud unless it committed nothing — the
//! idempotence a sync round must have.

use holon_pbt_core::RequiredWiring;
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefRemoteListSync;
use holon_pbt_core::capabilities::RemoteListMutation;
use holon_pbt_core::capabilities::SutRemoteListSync;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest::strategy::Union;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::remote_list_fixture::CAT;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

const AMOUNTS: &[&str] = &["2kg", "500g", "0,5 l", "4 Zehen", "1", "6"];

/// Draw weights. The other transitions together weigh over a thousand and a
/// case runs a few dozen steps at most, so a lower weight leaves most cases
/// with no round at all. An empty list weighs more so a case reaches its first
/// round early and the later arms have rows to work on.
const WEIGHT_EMPTY_LIST: u32 = 130;
const WEIGHT: u32 = 65;

/// Mutate the fixture peer's list and run one sync round, then a second round
/// that must be a no-op.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I mutate the remote list and sync one round {mutation}")]
pub struct RemoteListSync {
    pub mutation: RemoteListMutation,
}

impl TransitionFactory<ReferenceState> for RemoteListSync {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> RequiredWiring {
        // The round applies its local writes through the production dispatcher
        // into the persisted mirror table — pure Turso.
        RequiredWiring::HasStorage(StorageAdapter::Turso)
    }

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        vec![check(state.app_started(), Reason::AppNotStarted)]
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(move |_| {
                let next = state.remote_list.next_label();
                let existing = state.remote_list.existing_columns();
                let ticked_only = state.remote_list.ticked_only_columns();
                let existing_is_empty = existing.is_empty();
                let with_checked = |checked: &str| {
                    existing
                        .iter()
                        .filter(|columns| {
                            columns.iter().any(|(n, v)| n == "checked" && v == checked)
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                };
                let (active, ticked) = (with_checked("0"), with_checked("1"));
                let fresh_row = move |checked: Vec<&'static str>| {
                    (
                        // Amounts as a list app writes them: free text, a
                        // decimal comma, a bare number that must stay text.
                        prop::sample::select(AMOUNTS.to_vec()),
                        prop::sample::select(checked),
                    )
                        .prop_map(move |(count, checked)| {
                            vec![
                                ("name".to_string(), format!("item{next}")),
                                ("cat".to_string(), CAT.to_string()),
                                ("count".to_string(), count.to_string()),
                                ("checked".to_string(), checked.to_string()),
                            ]
                        })
                };
                // An amount a person types over the current one, keeping the
                // identity pair the edit addresses. The suffix tells the two
                // sides' edits apart.
                let amended = |columns: &[(String, String)], amount: &str, suffix: &str| {
                    columns
                        .iter()
                        .filter(|(name, _)| name == "name" || name == "cat")
                        .cloned()
                        .chain([("count".to_string(), format!("{amount} {suffix}"))])
                        .collect::<Vec<_>>()
                };
                let edit = |rows: Vec<Vec<(String, String)>>| {
                    (
                        prop::sample::select(rows),
                        prop::sample::select(AMOUNTS.to_vec()),
                    )
                        .prop_map(move |(columns, amount)| RemoteListMutation::EditLocally {
                            columns: amended(&columns, amount, "edited"),
                        })
                        .boxed()
                };
                let mut arms: Vec<(u32, BoxedStrategy<RemoteListMutation>)> = vec![
                    (
                        4,
                        fresh_row(vec!["0", "1"])
                            .prop_map(|columns| RemoteListMutation::Add { columns })
                            .boxed(),
                    ),
                    (
                        3,
                        fresh_row(vec!["0", "1"])
                            .prop_map(|columns| RemoteListMutation::AuthorLocally { columns })
                            .boxed(),
                    ),
                ];
                if !existing.is_empty() {
                    arms.push((
                        3,
                        prop::sample::select(existing.clone())
                            .prop_map(|columns| RemoteListMutation::Remove { columns })
                            .boxed(),
                    ));
                    arms.push((
                        2,
                        (
                            prop::sample::select(existing.clone()),
                            prop::sample::select(AMOUNTS.to_vec()),
                        )
                            .prop_map(move |(columns, amount)| RemoteListMutation::EditRemotely {
                                columns: amended(&columns, amount, "in app"),
                            })
                            .boxed(),
                    ));
                    arms.push((
                        2,
                        (
                            prop::sample::select(existing.clone()),
                            prop::sample::select(AMOUNTS.to_vec()),
                            prop::sample::select(AMOUNTS.to_vec()),
                        )
                            .prop_map(
                                move |(columns, mine, theirs)| RemoteListMutation::EditBoth {
                                    columns: amended(&columns, mine, "edited"),
                                    peer: amended(&columns, theirs, "app"),
                                },
                            )
                            .boxed(),
                    ));
                }
                // An item bought and then put back on the list from another
                // device: the app holds it active and ticked at once.
                if !ticked_only.is_empty() {
                    arms.push((
                        2,
                        prop::sample::select(ticked_only.clone())
                            .prop_map(|columns| RemoteListMutation::UntickRemotely { columns })
                            .boxed(),
                    ));
                    arms.push((
                        2,
                        prop::sample::select(ticked_only.clone())
                            .prop_map(|columns| RemoteListMutation::UntickLocally { columns })
                            .boxed(),
                    ));
                    arms.push((
                        2,
                        (
                            prop::sample::select(ticked_only),
                            prop::sample::select(AMOUNTS.to_vec()),
                        )
                            .prop_map(move |(columns, amount)| {
                                RemoteListMutation::AddActiveOverTicked {
                                    columns: amended(&columns, amount, "again"),
                                }
                            })
                            .boxed(),
                    ));
                }
                // The app names an item by its name, so a ticked entry and an
                // active one of that name fold into one row rather than refusing
                // the second. Only an active entry is duplicated.
                if !active.is_empty() {
                    // The later entry differs from the first in its amount, so
                    // a round that keeps the wrong one shows in the mirror.
                    arms.push((
                        2,
                        (
                            prop::sample::select(active.clone()),
                            prop::sample::select(AMOUNTS.to_vec()),
                        )
                            .prop_map(|(columns, amount)| RemoteListMutation::AddDuplicate {
                                columns: columns
                                    .into_iter()
                                    .map(|(name, value)| match name.as_str() {
                                        "count" => (name, format!("{amount} more")),
                                        _ => (name, value),
                                    })
                                    .collect(),
                            })
                            .boxed(),
                    ));
                    arms.push((2, edit(active.clone())));
                    arms.push((
                        2,
                        prop::sample::select(active.clone())
                            .prop_map(|columns| RemoteListMutation::TickLocally { columns })
                            .boxed(),
                    ));
                    arms.push((
                        2,
                        prop::sample::select(active)
                            .prop_map(|columns| RemoteListMutation::DeleteLocally { columns })
                            .boxed(),
                    ));
                }
                if !ticked.is_empty() {
                    arms.push((2, edit(ticked.clone())));
                    arms.push((
                        2,
                        prop::sample::select(ticked)
                            .prop_map(|columns| RemoteListMutation::DeleteLocally { columns })
                            .boxed(),
                    ));
                }
                let weight = if existing_is_empty {
                    WEIGHT_EMPTY_LIST
                } else {
                    WEIGHT
                };
                (
                    weight,
                    Union::new_weighted(arms)
                        .prop_map(|mutation| RemoteListSync { mutation })
                        .boxed(),
                )
            })
    }
}

impl TransitionRef<ReferenceState> for RemoteListSync {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![check(state.app_started(), Reason::AppNotStarted)]
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.remote_list.apply(&self.mutation);
    }
}

crate::cap_transition! {
    RemoteListSync: SutRemoteListSync,
    where R: [RefLifecycle + RefRemoteListSync],
    |me, state, sut| {
        sut.remote_list_mutate(me.mutation.clone()).await;
        let first = sut.remote_list_sync_round().await;
        let refusals = state.remote_list_expected_refusals();
        assert!(
            first.refused == refusals,
            "remote-list sync: the round reported {} refusal(s), the peer's list duplicates {} \
             key(s)",
            first.refused,
            refusals
        );
        let overwritten = state.remote_list_expected_overwritten();
        assert!(
            first.overwritten.len() == overwritten.len(),
            "remote-list sync: the round disclosed {:?} as overwritten, expected one disclosure \
             each for {overwritten:?}",
            first.overwritten
        );
        for (disclosure, (name, cat)) in first.overwritten.iter().zip(&overwritten) {
            let named = format!("`count` of {name} in {cat}: ");
            assert!(
                disclosure.starts_with(&named)
                    && !disclosure.contains('[')
                    && !disclosure.contains("Some("),
                "remote-list sync: the disclosure {disclosure:?} must open with {named:?}, the \
                 item as a person reads it, not its key or a debug rendering"
            );
        }
        // Idempotence: over an unchanged peer, a second round must decide
        // nothing to push. A round that re-pushes would re-send the same
        // command every poll.
        let second = sut.remote_list_sync_round().await;
        assert!(
            second.committed == 0,
            "remote-list sync: a second round over an unchanged peer committed {} command(s); \
             the round did not converge",
            second.committed
        );
        assert!(
            second.refused == refusals,
            "remote-list sync: the second round reported {} refusal(s), the first {}",
            second.refused,
            refusals
        );
        assert!(
            second.overwritten.is_empty(),
            "remote-list sync: a second round over an unchanged peer disclosed {:?} as overwritten",
            second.overwritten
        );
    }
    sql_budget: |_me, _state| {
        // Two rounds, each reading the mirror once; each round's local intents
        // land through the production dispatcher. The peer is in-process, so
        // its own reads never reach the span collector.
        ExpectedSql {
            reads: 2,
            writes: 8,
            ddl: 0,
            tolerance: 64,
        }
    }
}
