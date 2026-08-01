//! `inv-undo-redo-reference-heal`.
//!
//! @pbt oracle roundtrip
//! @pbt covers undo-redo-reference-heal — references to a block whose identity
//!   a `Redo` re-minted (undo deletes the tail, redo re-executes the forward op
//!   and mints a FRESH uuid; see `holon-core/src/undo.rs` and the ruling on
//!   `ReferenceState::pop_undo_to_redo`)
//! @pbt slips-if-removed a redo that brings a block back under a new id while
//!   every reference to its pre-undo id stays pointed at the burned uuid — the
//!   reference dangles permanently, and the DIAGNOSIS is lost: nothing else
//!   names the surviving reference row
//!
//! ## Overlap with `inv-focus-roots` — disclosed, not claimed away
//!
//! This is NOT the only invariant that reacts to the shape. When the surviving
//! reference is an open `navigation_history` row (a pin), `inv-focus-roots`
//! ALSO fires on the same tick (OBSERVED twice, 2026-07-26; see the parked case
//! in `hand-authored-regressions/keystone.jsonl`) — because the oracle resolves
//! the pin's synthetic label through the RE-POINTED reconcile map and therefore
//! predicts the healed id, while prod's base table still holds the burned one.
//!
//! What `inv-focus-roots` reports is "region X's focus-root set mismatches the
//! reference", classified as "a holon close-path / reference-model bug". That
//! names neither undo/redo nor the re-mint. What THIS invariant adds:
//!   * it fires only on a COMPLETED undo→redo round trip, so the mechanism is
//!     in the verdict rather than inferred from a set difference;
//!   * it enumerates the reference SITES (which store, which row, which dead
//!     id), across four stores, not just the focus-root projection;
//!   * it is ref-prediction-free — it compares the SUT against the burned-id
//!     record, so it still fires for sites the oracle does not model at all
//!     (marks, junction rows), where `inv-focus-roots` is structurally blind.
//!
//! ## Why this is a ROUND-TRIP property, not "no dangling references"
//!
//! A reference dangling immediately after an `Undo` is CORRECT: the block
//! genuinely does not exist, so pointing at nothing is the honest state. The
//! open question is whether the matching `Redo` HEALS it. Prod's redo
//! re-executes the stored forward op, so the block returns under a different
//! identity — and any reference written against the old identity stays dead.
//!
//! The separation is structural, not a heuristic: the invariant reads
//! [`RefUndoRedoBurned`], whose set the harness reconcile fills only for pairs
//! that are BOTH gone on the SUT side AND held again (under the same synthetic
//! label) on the oracle side. A bare undo cannot satisfy the second half — the
//! oracle dropped the label too — so nothing is asserted on an undo tick. The
//! set becomes non-empty exactly when the block has come back with a new id,
//! i.e. when the round trip has completed and healing was due.
//!
//! ## What it reads
//!
//! Only BASE-table truth, never a matview or a CDC mirror, so IVM lag cannot
//! manufacture a failure:
//!   - `block_raw.parent_id`               (`SutBackend::block_raw_snapshot`)
//!   - inline `Link` marks with an `Internal` target, over `block_raw.marks`
//!   - the `block_tags` junction           (`SutSqlProjection`)
//!   - open `navigation_history` rows      (`SutFocus`) — the pin / focus-root
//!     reference site
//!
//! `block_tags` is `(block_id TEXT, tag TEXT)`: the tag is a plain string, not
//! an id, so `block_id` is the ONLY block reference that junction can carry.
//! There is no second (target) column to miss.
//!
//! KNOWN BLIND SPOT, disclosed: the two junctions that DO hold block→block
//! references — `block_requires` and `advice_suppressed` — are not read here.
//! `SutBackend::block_raw_snapshot` deliberately omits junction-derived fields
//! (its own doc says so), and the only hydrated source is the CDC `block`
//! matview mirror, which would break the base-table-only sourcing above and let
//! IVM lag manufacture a failure. Closing this needs a base-table read on
//! `SutSqlProjection` (a `junction_reference_targets()` sibling of
//! `block_tag_block_ids`) implemented across its 4 production impls; until then
//! a `requires`/`advice_suppressed` row pointing at a burned id is INVISIBLE to
//! this invariant. It does not cause a false alarm, only an undercount.
//!
//! `block_history` is deliberately EXCLUDED: provenance rows are append-only by
//! design and are supposed to keep naming the burned id (see the C2
//! `history_*` correspondences, which assert exactly that).

use std::collections::BTreeSet;
use std::marker::PhantomData;

use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_pbt_core::capabilities::RefUndoRedoBurned;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutFocus;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// One surviving reference to a burned id: which store holds it, which row, and
/// which dead id it names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnhealedReference {
    pub site: &'static str,
    pub source: String,
    pub target: String,
}

/// Env var that flips the invariant from OBSERVE (report as `Skipped`, always
/// visible, never fatal) to ENFORCE (`Fail`). Observe is the default because
/// Martin ruled (2026-07-25) to KEEP the monotonic re-mint behaviour: this
/// invariant exists to MEASURE how often the consequence bites, not to fail a
/// build over a behaviour that is working as decided.
pub const ENFORCE_ENV: &str = "HOLON_PBT_UNDO_REDO_HEAL";

/// The ONLY accepted value of [`ENFORCE_ENV`]. Anything else panics — a typo
/// that silently downgraded an enforcing sweep to observe mode is exactly the
/// "looks fine, measured nothing" failure this invariant exists to catch.
pub const ENFORCE_VALUE: &str = "enforce";

pub struct InvUndoRedoReferenceHeal<R> {
    pub enforce: bool,
    pub _ref: PhantomData<R>,
}

impl<R> InvUndoRedoReferenceHeal<R> {
    pub const ID: InvariantId = InvariantId("inv-undo-redo-reference-heal");

    /// Enforce iff `HOLON_PBT_UNDO_REDO_HEAL=enforce`. Parse-don't-validate: an
    /// unset or empty variable is observe mode; the documented value is
    /// enforce; ANY other value panics naming what was passed, so a
    /// `=1`/`=true`/`=enforced` typo can never silently produce a
    /// non-enforcing run that the operator believes was enforcing.
    pub fn from_env() -> Self {
        let enforce = match std::env::var(ENFORCE_ENV) {
            Err(std::env::VarError::NotPresent) => false,
            Err(std::env::VarError::NotUnicode(v)) => panic!(
                "{ENFORCE_ENV} is not valid unicode ({v:?}); expected either unset or \
                 `{ENFORCE_VALUE}`"
            ),
            Ok(v) if v.is_empty() => false,
            Ok(v) if v == ENFORCE_VALUE => true,
            Ok(v) => panic!(
                "{ENFORCE_ENV}={v:?} is not a recognised value. The ONLY accepted value is \
                 `{ENFORCE_VALUE}`; unset (or empty) means observe mode. Refusing to run, because \
                 silently downgrading to observe would report a green sweep that enforced nothing."
            ),
        };
        Self {
            enforce,
            _ref: PhantomData,
        }
    }
}

/// Every reference site in `sut` that still names an id in `burned`.
pub async fn unhealed_references<S>(sut: &S, burned: &BTreeSet<EntityUri>) -> Vec<UnhealedReference>
where
    S: SutBackend + SutSqlProjection + SutFocus,
{
    let mut found: Vec<UnhealedReference> = Vec::new();

    for block in sut.block_raw_snapshot().await {
        if burned.contains(&block.parent_id) {
            found.push(UnhealedReference {
                site: "block_raw.parent_id",
                source: block.id.to_string(),
                target: block.parent_id.to_string(),
            });
        }
        for span in block.marks.iter().flatten() {
            let InlineMark::Link { target, .. } = &span.mark else {
                continue;
            };
            // Only a target that NAMES an entity can be a burned reference; a
            // colon-bearing page path names none.
            let Some(uri) = target.entity_uri() else {
                continue;
            };
            if burned.contains(&uri) {
                found.push(UnhealedReference {
                    site: "block_raw.marks[link]",
                    source: block.id.to_string(),
                    target: uri.to_string(),
                });
            }
        }
    }

    for id in sut.block_tag_block_ids().await {
        if burned.contains(&id) {
            found.push(UnhealedReference {
                site: "block_tags.block_id",
                source: id.to_string(),
                target: id.to_string(),
            });
        }
    }

    for (region, block_id) in sut.nav_history_open_rows().await {
        let parsed = EntityUri::parse(&block_id).unwrap_or_else(|e| {
            panic!("navigation_history.block_id '{block_id}' is not a valid EntityUri: {e}")
        });
        if burned.contains(&parsed) {
            found.push(UnhealedReference {
                site: "navigation_history(open).block_id",
                source: region,
                target: block_id,
            });
        }
    }

    found.sort();
    found
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvUndoRedoReferenceHeal<R>
where
    R: RefUndoRedoBurned,
    S: SutBackend + SutSqlProjection + SutFocus,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let burned = ref_.burned_block_ids();
        if burned.is_empty() {
            // NOT `Ok`: no undo→redo round trip has completed, so this tick
            // measured NOTHING. Reporting `Ok` would score the tick as
            // "engaged" in the vacuity dashboard and make an instrument that
            // never took a reading indistinguishable from one with real
            // coverage. `Skipped` is the honest verdict — `0/N` in the
            // engagement summary is the true statement.
            return InvariantResult::Skipped(
                "[inv-undo-redo-reference-heal] no completed undo→redo round trip on this tick \
                 (burned-id set empty) — nothing to check"
                    .to_string(),
            );
        }

        let unhealed = unhealed_references(sut, &burned).await;
        if unhealed.is_empty() {
            // The DENOMINATOR of the measurement: a round trip completed and
            // every reference to the burned identity was already gone or
            // re-pointed. Printed so a sweep can be read as a rate, not just a
            // pass/fail.
            eprintln!(
                "[inv-undo-redo-reference-heal] round-trip observed, references intact: burned={}",
                burned.len()
            );
            return InvariantResult::Ok;
        }

        let detail = unhealed
            .iter()
            .take(10)
            .map(|u| format!("{} {} -> {}", u.site, u.source, u.target))
            .collect::<Vec<_>>()
            .join("\n    ");
        let report = format!(
            "undo→redo did NOT restore referential integrity: {} reference(s) still name a block \
             id the redo burned. The block came back under a NEW identity, so these references \
             are now permanently dangling — a bare undo would be fine (the block is genuinely \
             absent), a redo is not.\n  burned ids: {:?}\n  unhealed (up to 10):\n    {detail}",
            unhealed.len(),
            burned.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
        );
        if self.enforce {
            return InvariantResult::Fail(format!("[inv-undo-redo-reference-heal] {report}"));
        }
        // The measurement channel. `Skipped` alone only moves the engagement
        // counter; the sweep operator needs the sites, so print them.
        let observed = format!(
            "[inv-undo-redo-reference-heal] {ENFORCE_ENV} off (Martin ruling 2026-07-25: keep the \
             monotonic re-mint, measure its cost) — OBSERVED, not enforced: {report}"
        );
        eprintln!("{observed}");
        InvariantResult::Skipped(observed)
    }
}
