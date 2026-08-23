//! `inv-no-declared-column-absent` — no subscription delivered a row short of
//! the declared columns its entity profile's computed fields require.
//!
//! ## Why this exists (the eight chronic boot warnings, 2026-08-24)
//! `warn_missing_declared_column` (`holon-api`'s computed-field evaluator) is
//! the LOUD half of type-aware binding: a computed field's required column is
//! in the entity's declared schema, but the row that reached the enrich seat
//! did not carry it. The field then renders as `Null` — which is how a
//! collapsed block drew the wrong bullet, `bullet_shape` having lost
//! `collapsed`. The signal is WARN-level, so `inv-no-observed-errors` (which
//! keys on ERROR) never saw it, and the count drifted from three to eight
//! unnoticed. See the bugfunnel entry
//! `2026-08-24-declared-column-absent-narrow-subscription-projections`.
//!
//! A boot-shaped test cannot hold this line: the narrow subscriptions live
//! behind the outline, and reaching them takes a navigation cursor and a
//! rendered embedded page. The composed transitions produce both, which is why
//! the guarantee belongs here.
//!
//! Ref-less, like [`crate::pbt::composed::observed_errors`]: the warnings live
//! in the global collector, not the reference `CapMap`.

use holon_pbt_core::composition::CapMap;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

use crate::test_tracing::SpanCollector;

/// The LOUD projection-gap signal. Matched on message text because the capture
/// layer flattens an event's fields into it, which also carries the
/// `context`/`column` pair into the failure message.
const SIGNAL: &str = "DECLARED column absent from row";

/// Read cap: the declared-column gaps captured since the last reset, each
/// rendered `computed_field/column`.
///
/// Deliberately the EXTRACTED pair and not the raw warning text. The check
/// below logs what it observes, that log is itself captured, and a payload
/// carrying [`SIGNAL`] verbatim would be re-read as a gap on the next
/// transition — the self-feeding escape loop `test_tracing` records from
/// 2026-07-11. The pair form cannot match the filter.
#[holon_macros::capmap_adapter]
pub trait DeclaredColumnGaps {
    fn declared_column_gaps(&self) -> Vec<String>;
}

/// Marks a warning that carried [`SIGNAL`] but whose `context`/`column` fields
/// could not be read. Deliberately carries NO raw text: the observe-mode log
/// would echo it back into the warning window and the filter would re-read it.
const MALFORMED: &str = "MALFORMED/unparseable-gap-warning";

/// `context="x" column="y"` → `x/y`. The capture layer flattens an event's
/// fields into its message, so the pair is recovered from the text.
///
/// A SIGNAL-carrying warning whose fields do not parse means the capture format
/// moved under us. Dropping it would make this oracle green forever while
/// `n/n` still read engaged — silent vacuity — so it is surfaced twice: as an
/// ERROR (which `inv-no-observed-errors` fails on, in BOTH modes, carrying the
/// raw text for diagnosis) and as [`MALFORMED`] in the returned set, which
/// fails enforce mode here.
fn gap_pair(message: &str) -> String {
    let field = |key: &str| {
        let head = message.find(&format!("{key}=\""))? + key.len() + 2;
        let rest = &message[head..];
        Some(&rest[..rest.find('"')?])
    };
    match (field("context"), field("column")) {
        (Some(context), Some(column)) => format!("{context}/{column}"),
        _ => {
            tracing::error!(
                raw = %message,
                "a declared-column warning matched the signal but its context/column fields \
                 did not parse — the capture format changed, and this oracle cannot read its \
                 own input. Fix the extractor before trusting any green from it."
            );
            MALFORMED.to_string()
        }
    }
}

/// Provider reading the process-global [`SpanCollector`]'s warning window.
#[derive(Default)]
pub struct ComposedDeclaredColumnGaps;

impl ComposedDeclaredColumnGaps {
    pub fn new() -> Self {
        Self
    }
}

impl DeclaredColumnGaps for ComposedDeclaredColumnGaps {
    fn declared_column_gaps(&self) -> Vec<String> {
        SpanCollector::global()
            .captured_warnings()
            .iter()
            .filter(|w| w.message.contains(SIGNAL))
            .map(|w| gap_pair(&w.message))
            .collect()
    }
}

/// Reports subscriptions that delivered rows short of their declared columns.
pub struct InvNoDeclaredColumnAbsent {
    enforce: bool,
}

impl InvNoDeclaredColumnAbsent {
    pub const ID: InvariantId = InvariantId("inv-no-declared-column-absent");

    /// Construct with the enforce flag read from
    /// `HOLON_PBT_DECLARED_COLUMN_ORACLE`.
    ///
    /// Observe-only by default because what counts as a violation is still
    /// open. Enforcing today would redden the keystone on ITS OWN generated
    /// projections: `crate::pbt::query::all_block_columns` omits `collapsed`
    /// and `widget_only`, so every `TestQuery::layout` trips this. That is not
    /// a test bug to paper over — it is the same question a user's deliberately
    /// narrow `live_query` raises, and
    /// `assets/default/types/block_profile.yaml` documents an absent
    /// `collapsed` as SAFE degradation to the plain bullet. Either that
    /// comment is wrong and the column is mandatory, or this over-reports a
    /// supported case.
    ///
    /// Decision card D7 settles it; `HOLON_PBT_DECLARED_COLUMN_ORACLE=enforce`
    /// is the one-line flip once it does.
    ///
    /// Reading the run output: the `inv-no-declared-column-absent=n/m` counter
    /// reports SELECTION (transitions where the cap was present), NOT how many
    /// gaps were seen. In observe-only mode the gaps appear as the
    /// `declared_column_oracle` WARN this check emits, not in that ratio.
    pub fn from_env() -> Self {
        Self {
            enforce: std::env::var("HOLON_PBT_DECLARED_COLUMN_ORACLE").as_deref() == Ok("enforce"),
        }
    }
}

#[allow(async_fn_in_trait)]
impl Invariant<CapMap, CapMap> for InvNoDeclaredColumnAbsent {
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &CapMap, sut: &CapMap) -> InvariantResult {
        let gaps = sut.declared_column_gaps();
        if gaps.is_empty() {
            return InvariantResult::Ok;
        }
        if self.enforce {
            InvariantResult::Fail(format!(
                "[inv-no-declared-column-absent] {} projection gap(s): a subscription delivered \
                 rows short of the declared columns its entity profile's computed fields require, \
                 so those fields rendered as Null. Widen the offending SELECT:\n  {}",
                gaps.len(),
                gaps.join("\n  "),
            ))
        } else {
            // No consumer reads a `Skipped` payload — the harness tallies the
            // disposition and discards the string, and `first_divergent`
            // renders it "skipped (observed nothing)", the exact inverse of
            // what a gap-carrying Skip means. So the observation is emitted
            // HERE, where it can still reach a log, or observe-only mode
            // observes into a void.
            tracing::warn!(
                target: "holon_pbt",
                stage = "declared_column_oracle",
                gaps = gaps.len() as u64,
                pairs = %gaps.join("; "),
                "inv-no-declared-column-absent OBSERVING (not enforcing): {} declared-column \
                 gap(s) this transition. Set HOLON_PBT_DECLARED_COLUMN_ORACLE=enforce to fail \
                 on these once D7 rules.",
                gaps.len(),
            );
            InvariantResult::Skipped(format!(
                "HOLON_PBT_DECLARED_COLUMN_ORACLE off (D7 open) — {} observed declared-column \
                 gap(s): {}",
                gaps.len(),
                gaps.join("; "),
            ))
        }
    }
}
