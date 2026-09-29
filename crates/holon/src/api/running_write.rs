//! The admission claim of the write this task runs, as the dispatcher deep
//! inside it sees it: a judged op claims the tagged blocks its judgement
//! shapes under it, and holds them until the write ends.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use futures::FutureExt;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::admission::Claim;
use holon_api::admission::ClaimRef;
use holon_api::admission::Footprint;
use holon_api::admission::NestRefused;
use holon_core::shape_gate::Judgement;

tokio::task_local! {
    static RUNNING_WRITE: Arc<RunningWrite>;
}

pub(crate) struct RunningWrite {
    claim: ClaimRef,
    ran: AtomicBool,
    refused: AtomicBool,
    /// The blocks of the write's shape claim, and that claim when `claim`
    /// does not cover them itself.
    shape: Mutex<(BTreeSet<EntityUri>, Option<Claim>)>,
    #[cfg(feature = "dispatch-hold")]
    hold: Arc<crate::api::dispatch_hold::DispatchHold>,
}

/// How one run of a write ended, as far as retrying it goes.
pub(crate) struct Attempt {
    /// A provider ran: the write may have landed in part.
    pub ran: bool,
    /// A shape claim of the write was refused.
    pub refused: bool,
}

/// What [`RunningWrite::claim_shape`] did.
pub(crate) enum ShapeClaim {
    /// The write's own claim covers the blocks: nothing else wrote them since
    /// the judgement.
    Covered,
    /// The write now holds the blocks, after every earlier write on them
    /// settled: what was judged may be stale.
    Waited,
}

impl RunningWrite {
    pub(crate) fn new(
        claim: ClaimRef,
        #[cfg(feature = "dispatch-hold")] hold: Arc<crate::api::dispatch_hold::DispatchHold>,
    ) -> Self {
        Self {
            claim,
            ran: AtomicBool::new(false),
            refused: AtomicBool::new(false),
            shape: Mutex::new((BTreeSet::new(), None)),
            #[cfg(feature = "dispatch-hold")]
            hold,
        }
    }

    /// Run `write` as this task's running write. Its shape claim settles
    /// before the caller drops the write's own claim.
    ///
    /// A combinator, not an `async fn`: an `async fn` keeps `write` beside
    /// the future it awaits, which doubles the size of the write's future.
    pub(crate) fn run<F: std::future::Future>(
        self,
        write: F,
    ) -> impl std::future::Future<Output = (F::Output, Attempt)> {
        let running = Arc::new(self);
        RUNNING_WRITE
            .scope(Arc::clone(&running), write)
            .map(move |output| {
                drop(running.shape.lock().expect("shape claim poisoned").1.take());
                let attempt = Attempt {
                    ran: running.ran.load(Ordering::SeqCst),
                    refused: running.refused.load(Ordering::SeqCst),
                };
                (output, attempt)
            })
    }

    pub(crate) fn current() -> Option<Arc<RunningWrite>> {
        // ALLOW(ok): the only error is "no running write in this task", which is
        // `None`.
        RUNNING_WRITE.try_with(Arc::clone).ok()
    }

    /// A provider is about to write.
    pub(crate) fn mark_ran(&self) {
        self.ran.store(true, Ordering::SeqCst);
    }

    pub(crate) fn ran(&self) -> bool {
        self.ran.load(Ordering::SeqCst)
    }

    pub(crate) fn seq(&self) -> holon_api::admission::AdmissionSeq {
        self.claim.seq()
    }

    pub(crate) fn holds(&self, blocks: &BTreeSet<EntityUri>) -> bool {
        blocks.is_subset(&self.shape.lock().expect("shape claim poisoned").0)
    }

    /// The blocks of `judgement`'s shape claim this write does not hold. A
    /// child of a tagged root it holds counts as held: every judged write that
    /// gives that root a child, or changes one, claims the root too.
    pub(crate) fn unheld(&self, judgement: &Judgement) -> BTreeSet<EntityUri> {
        let shape = self.shape.lock().expect("shape claim poisoned");
        let held = &shape.0;
        let under_held_root = |id: &EntityUri| {
            judgement.simulated.iter().any(|(root, children)| {
                held.contains(&root.id) && children.iter().any(|c| &c.id == id)
            })
        };
        judgement
            .shape_claim()
            .into_iter()
            .filter(|id| !held.contains(id) && !under_held_root(id))
            .collect()
    }

    /// Hold `wanted` together with the blocks already held, waiting for every
    /// earlier write on them. `op` names the judged op, for the test hold.
    pub(crate) async fn claim_shape(
        &self,
        wanted: &BTreeSet<EntityUri>,
        op: &str,
    ) -> Result<ShapeClaim, NestRefused> {
        let mut nested = {
            let mut shape = self.shape.lock().expect("shape claim poisoned");
            let blocks: BTreeSet<EntityUri> = shape.0.union(wanted).cloned().collect();
            #[cfg(feature = "dispatch-hold")]
            if self.hold.refuses_nest(&EntityName::new("block"), op) {
                self.refused.store(true, Ordering::SeqCst);
                return Err(NestRefused {
                    parent: self.claim.seq(),
                    through: self.claim.seq(),
                });
            }
            #[cfg(not(feature = "dispatch-hold"))]
            let _ = op;
            let footprint = Footprint::Subjects {
                relation: EntityName::new("block"),
                subjects: blocks.clone(),
            };
            if let Some(mut held) = shape.1.take() {
                let grown = held.grow(footprint);
                if let Err(refused) = grown {
                    shape.1 = Some(held);
                    self.refused.store(true, Ordering::SeqCst);
                    return Err(refused);
                }
                shape.0 = blocks;
                held
            } else {
                match self.claim.nest(footprint) {
                    Err(refused) => {
                        self.refused.store(true, Ordering::SeqCst);
                        return Err(refused);
                    }
                    Ok(None) => {
                        shape.0 = blocks;
                        return Ok(ShapeClaim::Covered);
                    }
                    Ok(Some(nested)) => {
                        shape.0 = blocks;
                        nested
                    }
                }
            }
        };
        nested.released().await;
        self.shape.lock().expect("shape claim poisoned").1 = Some(nested);
        Ok(ShapeClaim::Waited)
    }

    /// The test hold between a judged op's judgement and its first write.
    #[cfg(feature = "dispatch-hold")]
    pub(crate) async fn judged_checkpoint(&self, entity: &str, op: &str) -> anyhow::Result<()> {
        self.hold
            .judged_checkpoint(&EntityName::new(entity), op)
            .await
    }
}

#[cfg(all(test, feature = "dispatch-hold"))]
mod tests {
    use futures::FutureExt;
    use holon_api::admission::AdmissionOverlay;

    use super::*;

    fn on(ids: &[&str]) -> Footprint {
        Footprint::Subjects {
            relation: EntityName::new("block"),
            subjects: ids.iter().map(|id| EntityUri::block(id)).collect(),
        }
    }

    fn blocks(ids: &[&str]) -> BTreeSet<EntityUri> {
        ids.iter().map(|id| EntityUri::block(id)).collect()
    }

    /// `queued` waits on the write's shape claim of `t`. The claim grows to
    /// `u` too; `queued` still runs only after the whole write.
    #[tokio::test]
    async fn a_write_queued_behind_a_shape_claim_waits_while_it_grows() {
        let overlay = AdmissionOverlay::default();
        let write = overlay.admit(on(&["x"]));
        let running = RunningWrite::new(write.handle(), Default::default());

        running
            .claim_shape(&blocks(&["t"]), "set_field")
            .await
            .expect("no cycle");
        let mut queued = overlay.admit(on(&["t"]));
        assert!(queued.released().now_or_never().is_none());

        let grown = running
            .claim_shape(&blocks(&["u"]), "set_field")
            .now_or_never();
        assert!(
            queued.released().now_or_never().is_none(),
            "a write queued on t ran while the write's claim on t grew"
        );
        grown
            .expect("the grown claim waits on a write queued behind it")
            .expect("no cycle");
        assert!(
            queued.released().now_or_never().is_none(),
            "a write queued on t ran while the write held t"
        );
        assert!(running.holds(&blocks(&["t", "u"])));

        drop(running.shape.lock().unwrap().1.take());
        assert!(queued.released().now_or_never().is_some());
        drop(write);
    }
}
