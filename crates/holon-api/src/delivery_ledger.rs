//! Which CDC deliveries are still owed, and through which seq every view is
//! complete. A pure value: callers read their counters, feed them in, and ask.

use std::collections::BTreeMap;
use std::fmt;

/// A CDC batch sequence number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(pub u64);

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// One demux that routes the emitted batches on to its views.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DemuxId(pub u64);

/// One reporting view. Views of the same name are told apart by `instance`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ViewKey {
    pub name: String,
    pub instance: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct View {
    sent: Seq,
    /// The highest seq the view reported applied.
    reported: Seq,
    /// What the view's applies cover: never above what it was sent when it
    /// applied.
    applied: Option<Seq>,
    /// The seq the view was sent when its re-query failed, and why.
    failure: Option<(Seq, String)>,
    /// The seq the UI published while the view applied nothing yet.
    published: Option<Seq>,
}

impl View {
    fn unresolved_failure(&self) -> Option<&(Seq, String)> {
        self.failure
            .as_ref()
            .filter(|(at, _)| self.applied.is_none_or(|a| a < *at))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeliveryLedger {
    emitted: Seq,
    demuxes: BTreeMap<DemuxId, Seq>,
    views: BTreeMap<ViewKey, View>,
    watch_releases: u64,
    watch_registrations: u64,
}

fn advance(current: &mut Seq, to: Seq, what: &dyn fmt::Debug) {
    assert!(to >= *current, "{what:?} went back from {current} to {to}");
    *current = to;
}

impl DeliveryLedger {
    pub fn emitted(&mut self, seq: Seq) {
        advance(&mut self.emitted, seq, &"emitted");
    }

    pub fn routed(&mut self, demux: DemuxId, seq: Seq) {
        advance(self.demuxes.entry(demux).or_default(), seq, &demux);
    }

    pub fn drop_demux(&mut self, demux: DemuxId) {
        assert!(
            self.demuxes.remove(&demux).is_some(),
            "{demux:?} dropped but never routed"
        );
    }

    fn view(&mut self, key: ViewKey) -> &mut View {
        self.views.entry(key).or_insert(View {
            sent: Seq(0),
            reported: Seq(0),
            applied: None,
            failure: None,
            published: None,
        })
    }

    pub fn sent(&mut self, key: ViewKey, seq: Seq) {
        let what = format!("{key:?} sent");
        advance(&mut self.view(key).sent, seq, &what);
    }

    /// An apply above what the view was sent covers only what it was sent: a
    /// batch sent later stays owed until the view applies it.
    pub fn applied(&mut self, key: ViewKey, seq: Seq) {
        let what = format!("{key:?} applied");
        let view = self.view(key);
        advance(&mut view.reported, seq, &what);
        view.applied = Some(view.applied.unwrap_or_default().max(seq.min(view.sent)));
    }

    /// The UI published `seq` while the view applied nothing yet: the view
    /// holds [`Self::complete_through`] there.
    pub fn published(&mut self, key: ViewKey, seq: Seq) {
        let what = format!("{key:?} published");
        let view = self.view(key);
        advance(view.published.get_or_insert_default(), seq, &what);
    }

    /// The view's re-query failed after it was sent what it was sent so far.
    /// It stays owed until it applies through that seq.
    pub fn requery_failed(&mut self, key: ViewKey, cause: String) {
        let view = self.view(key);
        view.failure = Some((view.sent, cause));
    }

    pub fn drop_view(&mut self, key: &ViewKey) {
        assert!(
            self.views.remove(key).is_some(),
            "{key:?} dropped but never fed"
        );
    }

    pub fn watch_releases_began(&mut self, n: u64) {
        self.watch_releases += n;
    }

    pub fn watch_releases_ended(&mut self, n: u64) {
        self.watch_releases = self
            .watch_releases
            .checked_sub(n)
            .expect("more watch releases ended than began");
    }

    pub fn watch_registrations_began(&mut self, n: u64) {
        self.watch_registrations += n;
    }

    pub fn watch_registrations_ended(&mut self, n: u64) {
        self.watch_registrations = self
            .watch_registrations
            .checked_sub(n)
            .expect("more watch registrations ended than began");
    }

    /// The lowest seq every live demux routed through; with no demux, every
    /// emitted batch counts as routed.
    pub fn routed_through(&self) -> Seq {
        self.demuxes.values().copied().min().unwrap_or(self.emitted)
    }

    /// Every debt, by name: empty iff every live demux routed through the
    /// emitted seq, every view applied through what it was sent with no
    /// unresolved re-query failure, and no watch release or registration is
    /// in flight.
    pub fn owed(&self) -> Vec<Owed> {
        let mut owed = Vec::new();
        let routed = self.routed_through();
        if routed < self.emitted {
            owed.push(Owed::Routed {
                emitted: self.emitted,
                routed,
            });
        }
        for (key, view) in &self.views {
            let view_name = || key.name.clone();
            if let Some((at, cause)) = view.unresolved_failure() {
                owed.push(Owed::RequeryFailed {
                    view: view_name(),
                    at: *at,
                    applied: view.applied,
                    cause: cause.clone(),
                });
                continue;
            }
            match view.applied {
                None => owed.push(Owed::NotPrimed { view: view_name() }),
                Some(applied) if applied < view.sent => owed.push(Owed::Applied {
                    view: view_name(),
                    sent: view.sent,
                    applied,
                }),
                Some(_) => {}
            }
        }
        if self.watch_releases > 0 {
            owed.push(Owed::WatchReleases(self.watch_releases));
        }
        if self.watch_registrations > 0 {
            owed.push(Owed::WatchRegistrations(self.watch_registrations));
        }
        owed
    }

    /// The highest seq through which every view shows every routed batch. A
    /// view that applied nothing yet holds it at the seq the UI published for
    /// it.
    pub fn complete_through(&self) -> Seq {
        self.views
            .iter()
            .filter_map(|(key, view)| view.holds_at(key))
            .fold(self.routed_through(), Seq::min)
    }

    /// The highest seq through which the one view `key` shows every routed
    /// batch.
    pub fn complete_through_view(&self, key: &ViewKey) -> Seq {
        let view = self
            .views
            .get(key)
            .unwrap_or_else(|| panic!("{key:?} was never fed"));
        view.holds_at(key).map_or(self.routed_through(), |held| {
            held.min(self.routed_through())
        })
    }
}

impl View {
    /// Where the view holds completeness, `None` when it shows everything it
    /// was sent.
    fn holds_at(&self, key: &ViewKey) -> Option<Seq> {
        match self.applied {
            None => Some(self.published.unwrap_or_else(|| {
                panic!("{key:?} applied nothing and the UI published no seq for it")
            })),
            Some(applied) if applied < self.sent || self.unresolved_failure().is_some() => {
                Some(applied)
            }
            Some(_) => None,
        }
    }
}

/// What a settle read of the block feed's write-back consumer finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettleVerdict {
    Settled,
    /// The consumer has not applied through this emitted seq.
    FeedBehind(u64),
    WritebackOwes,
}

/// The settle read of the write-back feed. `start` is the seq emitted when the
/// read began, `None` without a database. `applied` reads the seq through
/// which the feed's view is complete; it is only valid for a taken cursor, so
/// it runs only then. A feed nothing took is not waited on.
pub fn settle_verdict(
    feed_taken: bool,
    start: Option<u64>,
    applied: impl FnOnce() -> Seq,
    writeback_owes_empty: impl FnOnce() -> bool,
) -> SettleVerdict {
    match start {
        Some(start) if feed_taken && applied() < Seq(start) => SettleVerdict::FeedBehind(start),
        _ if writeback_owes_empty() => SettleVerdict::Settled,
        _ => SettleVerdict::WritebackOwes,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    CdcRouted,
    CdcApplied,
    CdcPrimed,
    CdcRequery,
    WatchRelease,
    WatchRegister,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Stage::CdcRouted => "cdc-routed",
            Stage::CdcApplied => "cdc-applied",
            Stage::CdcPrimed => "cdc-primed",
            Stage::CdcRequery => "cdc-requery",
            Stage::WatchRelease => "watch-release",
            Stage::WatchRegister => "watch-register",
        }
    }
}

/// One delivery still owed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Owed {
    Routed {
        emitted: Seq,
        routed: Seq,
    },
    Applied {
        view: String,
        sent: Seq,
        applied: Seq,
    },
    NotPrimed {
        view: String,
    },
    RequeryFailed {
        view: String,
        at: Seq,
        applied: Option<Seq>,
        cause: String,
    },
    WatchReleases(u64),
    WatchRegistrations(u64),
}

impl Owed {
    pub fn stage(&self) -> Stage {
        match self {
            Owed::Routed { .. } => Stage::CdcRouted,
            Owed::Applied { .. } => Stage::CdcApplied,
            Owed::NotPrimed { .. } => Stage::CdcPrimed,
            Owed::RequeryFailed { .. } => Stage::CdcRequery,
            Owed::WatchReleases(_) => Stage::WatchRelease,
            Owed::WatchRegistrations(_) => Stage::WatchRegister,
        }
    }

    pub fn target(&self) -> String {
        match self {
            Owed::Routed { emitted, .. } => format!("emitted seq {emitted}"),
            Owed::Applied { view, sent, .. } => format!("sent seq {sent} to view {view}"),
            Owed::NotPrimed { view } => format!("a first applied batch in view {view}"),
            Owed::RequeryFailed { view, at, .. } => format!("applied seq {at} in view {view}"),
            Owed::WatchReleases(_) | Owed::WatchRegistrations(_) => "0 in flight".to_string(),
        }
    }

    pub fn value(&self) -> String {
        match self {
            Owed::Routed { routed, .. } => format!("routed seq {routed}"),
            Owed::Applied { applied, .. } => format!("applied seq {applied}"),
            Owed::NotPrimed { .. } => "none applied".to_string(),
            Owed::RequeryFailed { cause, .. } => format!("a failed re-query: {cause}"),
            Owed::WatchReleases(n) | Owed::WatchRegistrations(n) => format!("{n} in flight"),
        }
    }
}

impl fmt::Display for Owed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: target {}, at {}",
            self.stage().name(),
            self.target(),
            self.value()
        )
    }
}

#[cfg(test)]
mod tests;
