//! Concurrent-crossing arbitration (ADR 0028 H2 / D2).
//!
//! Concurrent divergent crossings on the same block are ordered
//! deterministically by the frozen tuple; the **max-key crossing wins**
//! ("latest crossing wins") and every other contender is **rejected loudly**
//! and returned as a *keepable divergent copy* — a real, materializable value
//! the caller can keep, not just an error string (review A4: YES).
//!
//! ## What "divergent / concurrent" means here (design of record)
//!
//! Concurrency is a *causal* property, not a lamport-ordering one (two entries
//! with different lamports can still be concurrent), so it cannot be recovered
//! from the total order alone. Each [`Crossing`] therefore carries an explicit
//! causal witness — [`Crossing::supersedes`], the crossing the author believed
//! was the block's current one when authoring:
//!
//! - A crossing that is named by another crossing's `supersedes` (a VALID,
//!   same-block witness) is a **superseded** prior state: legitimate history
//!   the later author observed and replaced. It is neither winner nor loser —
//!   it is accounted for in [`Arbitration::superseded`].
//! - The **live** crossings for a block are those no valid witness supersedes.
//!   One live crossing ⇒ uncontended winner. Two or more ⇒ mutually concurrent
//!   divergent crossings: the **max-key** one wins, the rest are loud-rejected
//!   losers with keepable copies (D2).
//!
//! ## Trust model + fail-loud witness validation (verifier fix)
//!
//! `supersedes` is **owner-scope-trusted input**: it is set by the owner's own
//! migration drivers, and the whole log lives inside one owner's vault. A
//! lying or buggy owner device can therefore only corrupt *its own* vault's
//! arbitration — never another owner's. But "trusted" must not mean "honored
//! blindly": a malformed witness (a migration-driver bug minting a wrong
//! `CrossingId`) previously superseded a crossing on a DIFFERENT block,
//! silently dropping it. That is exactly the fail-open-in-disguise the repo
//! forbids.
//!
//! So a witness is honored ONLY if it is well-formed, and a malformed one is
//! **rejected loudly**, never silently honored and never silently dropped:
//! - **unknown target** — `supersedes` names an id absent from the log;
//! - **wrong block** — `supersedes` names a crossing on a different block;
//! - **cycle** — the same-block witness chain loops (A supersedes B supersedes
//!   … supersedes A), which would otherwise erase every crossing in the loop.
//!
//! Each malformed crossing lands in [`Arbitration::malformed`] with a keepable
//! copy, and its witness edge is NOT honored (so it cannot supersede anything).
//! Validation runs at BOTH the source
//! ([`crate::log::CrossingLog::append_crossing`]) and here at the read boundary
//! (defense against malformed entries arriving via merge from a peer device
//! that did not validate).
//!
//! ## Totality (the invariant that catches this bug class wholesale)
//!
//! `arbitrate` partitions its input `CrossingBegin` set into four disjoint,
//! exhaustive outcomes — **winners ⊎ losers ⊎ malformed ⊎ superseded = input**
//! — and asserts it before returning. No crossing may vanish; losers and
//! malformed each carry a keepable copy. Arbitration reads `CrossingBegin`
//! entries only; `CrossingCommit` brackets and `PolicyEdit`s drive the
//! migration/projection lifecycle, not winner selection.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::types::BlockId;
use crate::types::ContainerId;
use crate::types::Crossing;
use crate::types::CrossingId;
use crate::types::CrossingKey;
use crate::types::LogEntry;
use crate::types::LogEntryBody;

/// A loud-rejected losing crossing (D2), carrying a keepable divergent copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedCrossing {
    pub crossing: Crossing,
    /// The winning crossing's key this one lost to (always `> self`'s key).
    pub lost_to: CrossingKey,
    /// Loud, human-readable reason (block id + both keys) for disclosure.
    pub reason: String,
    /// The real value the caller can materialize instead of silently dropping.
    pub keepable_copy: DivergentCopy,
}

/// A crossing rejected as MALFORMED because its `supersedes` witness is invalid
/// (unknown id, different block, or on a cycle). Carries a keepable copy — a
/// malformed crossing is loud-rejected, never silently dropped, and its witness
/// is never silently honored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MalformedCrossing {
    pub crossing: Crossing,
    /// Loud reason naming the specific witness defect.
    pub reason: String,
    pub keepable_copy: DivergentCopy,
}

/// A materializable copy of a rejected crossing's block, so the loser is a
/// value the caller keeps — not data that vanished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DivergentCopy {
    pub block: BlockId,
    pub intended_target: ContainerId,
    pub content: crate::types::BlockContent,
}

/// The outcome of arbitrating a merged log. TOTALITY: every input
/// `CrossingBegin` lands in exactly one of the four disjoint vectors —
/// `winners ⊎ losers ⊎ malformed ⊎ superseded = input`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Arbitration {
    /// Crossings that stand — the max-key live crossing per contended block
    /// plus every uncontended live crossing.
    pub winners: Vec<Crossing>,
    /// Loud-rejected concurrent-divergent losers, each with a keepable copy.
    pub losers: Vec<RejectedCrossing>,
    /// Loud-rejected MALFORMED crossings (invalid `supersedes` witness), each
    /// with a keepable copy.
    pub malformed: Vec<MalformedCrossing>,
    /// Superseded prior history — legitimate crossings a later same-block
    /// crossing observed and replaced. Accounted for (not vanished), neither
    /// current-winner nor rejected.
    pub superseded: Vec<Crossing>,
}

impl Arbitration {
    pub fn is_winner(&self, id: &CrossingId) -> bool {
        self.winners.iter().any(|c| &c.crossing_id == id)
    }
    pub fn is_loser(&self, id: &CrossingId) -> bool {
        self.losers.iter().any(|r| &r.crossing.crossing_id == id)
    }
    pub fn is_malformed(&self, id: &CrossingId) -> bool {
        self.malformed.iter().any(|m| &m.crossing.crossing_id == id)
    }
    pub fn is_superseded(&self, id: &CrossingId) -> bool {
        self.superseded.iter().any(|c| &c.crossing_id == id)
    }
    /// Number of input crossings accounted for across all four outcomes.
    pub fn accounted(&self) -> usize {
        self.winners.len() + self.losers.len() + self.malformed.len() + self.superseded.len()
    }
}

/// Why a `supersedes` witness is malformed. Kept as a typed value so the same
/// classifier serves both source-side (`append_crossing`) and read-boundary
/// (`arbitrate`) validation.
pub(crate) enum WitnessDefect {
    UnknownTarget,
    WrongBlock { target_block: String },
    Cycle,
}

impl WitnessDefect {
    fn reason(&self, c: &Crossing) -> String {
        let sid = c
            .supersedes
            .as_ref()
            .map(|s| s.0.as_str())
            .unwrap_or("<none>");
        match self {
            WitnessDefect::UnknownTarget => format!(
                "crossing `{}` on block `{}` supersedes unknown crossing `{}` — \
                 witness references an id absent from the log (malformed; \
                 rejected loudly, a keepable copy is returned)",
                c.crossing_id.0, c.block.0, sid,
            ),
            WitnessDefect::WrongBlock { target_block } => format!(
                "crossing `{}` on block `{}` supersedes `{}` which is on a \
                 DIFFERENT block `{}` — a witness may only supersede a crossing \
                 of the SAME block (malformed; rejected loudly, not honored, a \
                 keepable copy is returned)",
                c.crossing_id.0, c.block.0, sid, target_block,
            ),
            WitnessDefect::Cycle => format!(
                "crossing `{}` on block `{}` is on a supersedes CYCLE (its \
                 same-block witness chain loops) — honoring it would erase every \
                 crossing in the loop (malformed; rejected loudly, a keepable \
                 copy is returned)",
                c.crossing_id.0, c.block.0,
            ),
        }
    }
}

/// Classify a single crossing's witness against the id→crossing index, ignoring
/// cycles (which need the whole graph). `None` = the witness is `None` or a
/// valid same-block reference.
pub(crate) fn classify_witness<'a>(
    c: &Crossing,
    by_id: &BTreeMap<&'a CrossingId, &'a Crossing>,
) -> Option<WitnessDefect> {
    let sid = c.supersedes.as_ref()?;
    match by_id.get(sid) {
        None => Some(WitnessDefect::UnknownTarget),
        Some(target) if target.block != c.block => Some(WitnessDefect::WrongBlock {
            target_block: target.block.0.clone(),
        }),
        Some(_) => None,
    }
}

fn divergent_copy(c: &Crossing) -> DivergentCopy {
    DivergentCopy {
        block: c.block.clone(),
        intended_target: c.target.clone(),
        content: c.content.clone(),
    }
}

/// Arbitrate a merged, key-sorted log. Deterministic and TOTAL: a pure function
/// of the entry set that accounts for every input crossing.
pub fn arbitrate(entries: &[LogEntry]) -> Arbitration {
    // Collect every CrossingBegin with its key.
    let mut begins: Vec<(&Crossing, CrossingKey)> = Vec::new();
    for e in entries {
        if let LogEntryBody::CrossingBegin(c) = &e.body {
            begins.push((c, e.key));
        }
    }
    let by_id: BTreeMap<&CrossingId, &Crossing> =
        begins.iter().map(|(c, _)| (&c.crossing_id, *c)).collect();

    // Phase 1: unknown-target / wrong-block malformed.
    let mut malformed: BTreeMap<&CrossingId, WitnessDefect> = BTreeMap::new();
    for (c, _) in &begins {
        if let Some(defect) = classify_witness(c, &by_id) {
            malformed.insert(&c.crossing_id, defect);
        }
    }

    // Phase 2: cycle detection over VALID same-block edges of not-yet-malformed
    // nodes (functional graph, out-degree ≤ 1). Any node on a cycle → malformed.
    let edge: BTreeMap<&CrossingId, &CrossingId> = begins
        .iter()
        .filter_map(|(c, _)| {
            if malformed.contains_key(&c.crossing_id) {
                return None;
            }
            c.supersedes.as_ref().map(|sid| (&c.crossing_id, sid))
        })
        .collect();
    for start in edge.keys() {
        mark_cycles_from(start, &edge, &mut malformed);
    }

    // Phase 3: superseded = ids named by a VALID witness of a NON-malformed
    // source (a malformed source's edge is never honored).
    let superseded: BTreeSet<&CrossingId> = begins
        .iter()
        .filter_map(|(c, _)| {
            if malformed.contains_key(&c.crossing_id) {
                return None;
            }
            c.supersedes.as_ref()
        })
        .collect();

    // Phase 4: bucket every crossing (precedence malformed > superseded > live).
    let mut out = Arbitration::default();
    let mut per_block: BTreeMap<&BlockId, Vec<(&Crossing, CrossingKey)>> = BTreeMap::new();
    for (c, key) in &begins {
        if let Some(defect) = malformed.get(&c.crossing_id) {
            out.malformed.push(MalformedCrossing {
                crossing: (*c).clone(),
                reason: defect.reason(c),
                keepable_copy: divergent_copy(c),
            });
        } else if superseded.contains(&c.crossing_id) {
            out.superseded.push((*c).clone());
        } else {
            per_block.entry(&c.block).or_default().push((*c, *key));
        }
    }

    for (_block, mut live) in per_block {
        live.sort_by_key(|(_, k)| *k);
        let (winner, winner_key) = live.pop().expect("a block group is never empty");
        for (loser, loser_key) in live {
            let reason = format!(
                "crossing `{}` on block `{}` (key lamport={},peer={}) LOST to \
                 `{}` (key lamport={},peer={}) — concurrent divergent crossing \
                 rejected (ADR 0028 D2, latest crossing wins); a keepable \
                 divergent copy targeting `{}` is returned",
                loser.crossing_id.0,
                loser.block.0,
                loser_key.lamport,
                loser_key.peer.0,
                winner.crossing_id.0,
                winner_key.lamport,
                winner_key.peer.0,
                loser.target.0,
            );
            out.losers.push(RejectedCrossing {
                crossing: loser.clone(),
                lost_to: winner_key,
                reason,
                keepable_copy: divergent_copy(loser),
            });
        }
        out.winners.push(winner.clone());
    }

    // TOTALITY: every input crossing is accounted for in exactly one outcome.
    assert_eq!(
        out.accounted(),
        begins.len(),
        "arbitration totality violated: {} inputs but {} accounted \
         (winners={}, losers={}, malformed={}, superseded={}) — a crossing was \
         silently dropped",
        begins.len(),
        out.accounted(),
        out.winners.len(),
        out.losers.len(),
        out.malformed.len(),
        out.superseded.len(),
    );
    out
}

/// Walk the functional supersedes graph from `start`; if the walk revisits a
/// node still on the current path, every node in that loop is marked malformed
/// (cycle). Nodes are followed only through `edge`, which already excludes
/// unknown/wrong-block-malformed sources.
fn mark_cycles_from<'a>(
    start: &'a CrossingId,
    edge: &BTreeMap<&'a CrossingId, &'a CrossingId>,
    malformed: &mut BTreeMap<&'a CrossingId, WitnessDefect>,
) {
    let mut path: Vec<&CrossingId> = Vec::new();
    let mut seen: BTreeSet<&CrossingId> = BTreeSet::new();
    let mut cur: &CrossingId = start;
    loop {
        if malformed.contains_key(cur) {
            return;
        }
        if seen.contains(cur) {
            // Found a loop: everything from the first occurrence of `cur` is a
            // cycle member.
            if let Some(pos) = path.iter().position(|x| *x == cur) {
                for node in &path[pos..] {
                    malformed.entry(node).or_insert(WitnessDefect::Cycle);
                }
            }
            return;
        }
        seen.insert(cur);
        path.push(cur);
        match edge.get(cur) {
            Some(next) => cur = next,
            None => return, // reaches a live root — no cycle on this walk
        }
    }
}
