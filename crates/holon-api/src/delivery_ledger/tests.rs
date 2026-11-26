use proptest::prelude::*;

use super::*;

const DEMUXES: u8 = 3;
/// Two views share each name, so a view is told apart by its instance.
const VIEWS: u8 = 4;
const CAUSES: [&str; 2] = ["no such table: block", "database is locked"];

/// One ledger input. Seqs are deltas onto the key's current seq, so every
/// draw feeds a seq that never goes back.
#[derive(Clone, Debug)]
enum Input {
    Emitted(u64),
    Routed(u8, u64),
    DropDemux(u8),
    Sent(u8, u64),
    Applied(u8, u64),
    Published(u8, u64),
    RequeryFailed(u8, usize),
    DropView(u8),
    WatchRelease(Delta),
    WatchRegistration(Delta),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delta {
    Began,
    Ended,
}

fn delta() -> impl Strategy<Value = Delta> {
    prop_oneof![Just(Delta::Began), Just(Delta::Ended)]
}

fn input() -> impl Strategy<Value = Input> {
    prop_oneof![
        (0u64..4).prop_map(Input::Emitted),
        (0..DEMUXES, 0u64..4).prop_map(|(d, by)| Input::Routed(d, by)),
        (0..DEMUXES).prop_map(Input::DropDemux),
        (0..VIEWS, 0u64..4).prop_map(|(v, by)| Input::Sent(v, by)),
        (0..VIEWS, 0u64..4).prop_map(|(v, by)| Input::Applied(v, by)),
        (0..VIEWS, 0u64..4).prop_map(|(v, by)| Input::Published(v, by)),
        (0..VIEWS, 0..CAUSES.len()).prop_map(|(v, c)| Input::RequeryFailed(v, c)),
        (0..VIEWS).prop_map(Input::DropView),
        delta().prop_map(Input::WatchRelease),
        delta().prop_map(Input::WatchRegistration),
    ]
}

fn demux(d: u8) -> DemuxId {
    DemuxId(u64::from(d))
}

fn view(v: u8) -> ViewKey {
    ViewKey {
        name: format!("view-{}", v % 2),
        instance: u64::from(v),
    }
}

/// One input with its absolute seq, as the model keeps it.
#[derive(Clone, Debug)]
enum Event {
    Emitted(Seq),
    Routed(DemuxId, Seq),
    DropDemux(DemuxId),
    Sent(ViewKey, Seq),
    Applied(ViewKey, Seq),
    Published(ViewKey, Seq),
    RequeryFailed(ViewKey, String),
    DropView(ViewKey),
    WatchRelease(Delta),
    WatchRegistration(Delta),
}

/// The oracle: keeps every event and recomputes from the whole history.
#[derive(Default)]
struct Model {
    events: Vec<Event>,
}

/// One view as the model rebuilds it from its events since its last drop.
struct ModelView {
    sent: Seq,
    /// The highest seq the view reported applied.
    reported: Option<Seq>,
    /// The highest seq an apply completes: an apply never completes the view
    /// beyond what it was sent before that apply.
    applied: Option<Seq>,
    failure: Option<(Seq, String)>,
    published: Option<Seq>,
}

impl Model {
    fn emitted(&self) -> Seq {
        self.events
            .iter()
            .filter_map(|e| match e {
                Event::Emitted(s) => Some(*s),
                _ => None,
            })
            .max()
            .unwrap_or_default()
    }

    /// Events about `d` after its last drop.
    fn demux_events(&self, d: DemuxId) -> Vec<Seq> {
        let mut routed = Vec::new();
        for e in &self.events {
            match e {
                Event::Routed(id, s) if *id == d => routed.push(*s),
                Event::DropDemux(id) if *id == d => routed.clear(),
                _ => {}
            }
        }
        routed
    }

    fn live_demuxes(&self) -> Vec<(DemuxId, Seq)> {
        (0..DEMUXES)
            .map(demux)
            .filter_map(|d| self.demux_events(d).last().map(|s| (d, *s)))
            .collect()
    }

    /// The events about `key` since its last drop; `None` if it has none.
    fn view_events(&self, key: &ViewKey) -> Option<Vec<&Event>> {
        let mut events: Option<Vec<&Event>> = None;
        for e in &self.events {
            match e {
                Event::Sent(k, _)
                | Event::Applied(k, _)
                | Event::Published(k, _)
                | Event::RequeryFailed(k, _)
                    if k == key =>
                {
                    events.get_or_insert_default().push(e)
                }
                Event::DropView(k) if k == key => events = None,
                _ => {}
            }
        }
        events
    }

    fn view_state(&self, key: &ViewKey) -> Option<ModelView> {
        let events = self.view_events(key)?;
        let sent_before = |i: usize| {
            events[..i]
                .iter()
                .filter_map(|e| match e {
                    Event::Sent(_, s) => Some(*s),
                    _ => None,
                })
                .max()
                .unwrap_or_default()
        };
        let applies = || {
            events.iter().enumerate().filter_map(|(i, e)| match e {
                Event::Applied(_, s) => Some((i, *s)),
                _ => None,
            })
        };
        Some(ModelView {
            sent: sent_before(events.len()),
            reported: applies().map(|(_, s)| s).max(),
            applied: applies().map(|(i, s)| s.min(sent_before(i))).max(),
            failure: events.iter().enumerate().rev().find_map(|(i, e)| match e {
                Event::RequeryFailed(_, cause) => Some((sent_before(i), cause.clone())),
                _ => None,
            }),
            published: events
                .iter()
                .filter_map(|e| match e {
                    Event::Published(_, s) => Some(*s),
                    _ => None,
                })
                .max(),
        })
    }

    fn live_views(&self) -> Vec<(ViewKey, ModelView)> {
        (0..VIEWS)
            .map(view)
            .filter_map(|k| self.view_state(&k).map(|v| (k, v)))
            .collect()
    }

    fn in_flight(&self, pick: impl Fn(&Event) -> Option<Delta>) -> u64 {
        self.events.iter().filter_map(pick).fold(0, |n, d| match d {
            Delta::Began => n + 1,
            Delta::Ended => n - 1,
        })
    }

    fn releases(&self) -> u64 {
        self.in_flight(|e| match e {
            Event::WatchRelease(d) => Some(*d),
            _ => None,
        })
    }

    fn registrations(&self) -> u64 {
        self.in_flight(|e| match e {
            Event::WatchRegistration(d) => Some(*d),
            _ => None,
        })
    }

    fn routed_through(&self) -> Seq {
        self.live_demuxes()
            .into_iter()
            .map(|(_, s)| s)
            .min()
            .unwrap_or(self.emitted())
    }

    fn unresolved(v: &ModelView) -> Option<&(Seq, String)> {
        v.failure
            .as_ref()
            .filter(|(at, _)| v.applied.is_none_or(|a| a < *at))
    }

    fn owed(&self) -> Vec<Owed> {
        let mut owed = Vec::new();
        let (emitted, routed) = (self.emitted(), self.routed_through());
        if routed < emitted {
            owed.push(Owed::Routed { emitted, routed });
        }
        for (key, v) in self.live_views() {
            if let Some((at, cause)) = Self::unresolved(&v) {
                owed.push(Owed::RequeryFailed {
                    view: key.name,
                    at: *at,
                    applied: v.applied,
                    cause: cause.clone(),
                });
                continue;
            }
            match v.applied {
                None => owed.push(Owed::NotPrimed { view: key.name }),
                Some(applied) if applied < v.sent => owed.push(Owed::Applied {
                    view: key.name,
                    sent: v.sent,
                    applied,
                }),
                Some(_) => {}
            }
        }
        if self.releases() > 0 {
            owed.push(Owed::WatchReleases(self.releases()));
        }
        if self.registrations() > 0 {
            owed.push(Owed::WatchRegistrations(self.registrations()));
        }
        owed.sort();
        owed
    }

    /// `None` when a view that applied nothing has no published seq: the
    /// ledger must refuse to answer.
    fn floor(&self) -> Option<Seq> {
        self.live_views()
            .into_iter()
            .filter_map(|(_, v)| Self::held(&v))
            .try_fold(self.routed_through(), |floor, held| Some(floor.min(held?)))
    }

    /// Where one view holds the floor; `Some(None)` when it applied nothing
    /// and nothing was published for it.
    fn held(v: &ModelView) -> Option<Option<Seq>> {
        match v.applied {
            None => Some(v.published),
            Some(a) if a < v.sent || Self::unresolved(v).is_some() => Some(Some(a)),
            Some(_) => None,
        }
    }
}

/// Feeds `input` to the ledger and the model alike. `None` for an input the
/// ledger must refuse loudly (a drop of nothing, a release that never
/// began); the loud cases have their own property below.
fn resolve(model: &Model, input: &Input) -> Option<Event> {
    let current_demux = |d: DemuxId| model.demux_events(d).last().copied().unwrap_or_default();
    let current_view = |k: &ViewKey| model.view_state(k);
    Some(match *input {
        Input::Emitted(by) => Event::Emitted(Seq(model.emitted().0 + by)),
        Input::Routed(d, by) => Event::Routed(demux(d), Seq(current_demux(demux(d)).0 + by)),
        Input::DropDemux(d) => {
            model.demux_events(demux(d)).last()?;
            Event::DropDemux(demux(d))
        }
        Input::Sent(v, by) => {
            let sent = current_view(&view(v)).map_or(Seq(0), |s| s.sent);
            Event::Sent(view(v), Seq(sent.0 + by))
        }
        Input::Applied(v, by) => {
            let reported = current_view(&view(v)).and_then(|s| s.reported);
            Event::Applied(view(v), Seq(reported.unwrap_or_default().0 + by))
        }
        Input::Published(v, by) => {
            let published = current_view(&view(v)).and_then(|s| s.published);
            Event::Published(view(v), Seq(published.unwrap_or_default().0 + by))
        }
        Input::RequeryFailed(v, c) => Event::RequeryFailed(view(v), CAUSES[c].to_string()),
        Input::DropView(v) => {
            current_view(&view(v))?;
            Event::DropView(view(v))
        }
        Input::WatchRelease(d) => {
            if d == Delta::Ended && model.releases() == 0 {
                return None;
            }
            Event::WatchRelease(d)
        }
        Input::WatchRegistration(d) => {
            if d == Delta::Ended && model.registrations() == 0 {
                return None;
            }
            Event::WatchRegistration(d)
        }
    })
}

fn feed(ledger: &mut DeliveryLedger, event: &Event) {
    match event.clone() {
        Event::Emitted(s) => ledger.emitted(s),
        Event::Routed(d, s) => ledger.routed(d, s),
        Event::DropDemux(d) => ledger.drop_demux(d),
        Event::Sent(k, s) => ledger.sent(k, s),
        Event::Applied(k, s) => ledger.applied(k, s),
        Event::Published(k, s) => ledger.published(k, s),
        Event::RequeryFailed(k, cause) => ledger.requery_failed(k, cause),
        Event::DropView(k) => ledger.drop_view(&k),
        Event::WatchRelease(Delta::Began) => ledger.watch_releases_began(1),
        Event::WatchRelease(Delta::Ended) => ledger.watch_releases_ended(1),
        Event::WatchRegistration(Delta::Began) => ledger.watch_registrations_began(1),
        Event::WatchRegistration(Delta::Ended) => ledger.watch_registrations_ended(1),
    }
}

fn sorted(mut owed: Vec<Owed>) -> Vec<Owed> {
    owed.sort();
    owed
}

proptest! {
    /// After every input the ledger owes exactly what the full history owes,
    /// and its floor is the history's floor. Each view with an unresolved
    /// re-query failure is owed by name with its cause, and no floor rises
    /// above a lagging view's applied seq.
    #[test]
    fn the_ledger_owes_what_the_full_history_owes(
        inputs in proptest::collection::vec(input(), 0..60),
    ) {
        let mut ledger = DeliveryLedger::default();
        let mut model = Model::default();
        for input in &inputs {
            let Some(event) = resolve(&model, input) else {
                continue;
            };
            feed(&mut ledger, &event);
            model.events.push(event);

            prop_assert_eq!(sorted(ledger.owed()), model.owed());
            prop_assert_eq!(ledger.routed_through(), model.routed_through());
            let floor = std::panic::catch_unwind(|| ledger.complete_through()).ok();
            prop_assert_eq!(floor, model.floor());

            for (key, v) in model.live_views() {
                let view_floor = std::panic::catch_unwind(|| ledger.complete_through_view(&key)).ok();
                let expected = match Model::held(&v) {
                    None => Some(model.routed_through()),
                    Some(held) => held.map(|h| h.min(model.routed_through())),
                };
                prop_assert_eq!(view_floor, expected, "{:?}: the one view's floor", key);
                if let (Some(applied), Some(floor)) = (v.applied.filter(|a| *a < v.sent), floor) {
                    prop_assert!(floor <= applied, "{key:?} lags at {applied:?}, floor {floor:?}");
                }
                if let Some((at, cause)) = Model::unresolved(&v) {
                    let named = ledger.owed().into_iter().any(|o| {
                        o == Owed::RequeryFailed {
                            view: key.name.clone(),
                            at: *at,
                            applied: v.applied,
                            cause: cause.clone(),
                        }
                    });
                    prop_assert!(named, "{key:?} failed with {cause:?} but is not owed by it");
                }
            }
            prop_assert_eq!(
                ledger.owed().is_empty(),
                model.live_demuxes().iter().all(|(_, s)| *s >= model.emitted())
                    && model.live_views().iter().all(|(_, v)| {
                        v.applied.is_some_and(|a| a >= v.sent) && Model::unresolved(v).is_none()
                    })
                    && model.releases() == 0
                    && model.registrations() == 0
            );
        }
    }

    /// A seq that goes back for the same key, a drop of a key the ledger does
    /// not hold, and an end of something that never began each panic.
    #[test]
    fn a_seq_going_back_or_an_unknown_drop_panics(
        start in 1u64..50,
        back in 1u64..50,
        case in 0usize..9,
    ) {
        let back = Seq(start.saturating_sub(back));
        let outcome = std::panic::catch_unwind(|| {
            let mut ledger = DeliveryLedger::default();
            let key = view(0);
            match case {
                0 => {
                    ledger.emitted(Seq(start));
                    ledger.emitted(back);
                }
                1 => {
                    ledger.routed(demux(0), Seq(start));
                    ledger.routed(demux(0), back);
                }
                2 => {
                    ledger.sent(key.clone(), Seq(start));
                    ledger.sent(key, back);
                }
                3 => {
                    ledger.applied(key.clone(), Seq(start));
                    ledger.applied(key, back);
                }
                4 => {
                    ledger.published(key.clone(), Seq(start));
                    ledger.published(key, back);
                }
                5 => ledger.drop_demux(demux(0)),
                6 => ledger.drop_view(&key),
                7 => ledger.watch_releases_ended(1),
                _ => ledger.watch_registrations_ended(1),
            }
        });
        prop_assert!(outcome.is_err(), "case {case} did not panic");
    }

    /// An apply above what a view was sent never covers a batch sent to it
    /// later: the view owes that batch, unless the apply itself panics.
    #[test]
    fn an_apply_never_completes_a_view_beyond_what_it_was_sent(
        (v, sent, ahead, later) in applied_ahead_of_sent(),
    ) {
        let outcome = std::panic::catch_unwind(|| {
            let mut ledger = DeliveryLedger::default();
            ledger.sent(view(v), Seq(sent));
            ledger.applied(view(v), Seq(ahead));
            ledger.sent(view(v), Seq(later));
            ledger.owed()
        });
        if let Ok(owed) = outcome {
            prop_assert!(
                owed.iter().any(|o| matches!(o, Owed::Applied { .. })),
                "applied {ahead} after sent {sent} covers the later send {later}: owed {owed:?}"
            );
        }
    }
}

/// A view sent `sent`, an apply `ahead` above it, then a later send at or
/// below that apply.
fn applied_ahead_of_sent() -> impl Strategy<Value = (u8, u64, u64, u64)> {
    (0..VIEWS, 0u64..20, 1u64..20)
        .prop_flat_map(|(v, sent, by)| (Just(v), Just(sent), Just(sent + by), sent + 1..=sent + by))
}
