//! `inv-no-declared-column-absent` — no subscription delivered a row short of
//! the declared columns its entity profile's computed fields require.
//!
//! ## Why this exists (the eight chronic boot warnings, 2026-08-24)
//! `warn_missing_declared_column` (`holon-api`'s computed-field evaluator) is
//! the LOUD half of type-aware binding: a computed field's required column is
//! in the entity's declared schema AND the renderer bound to the subscription
//! needs it, but the row that reached the enrich seat did not carry it. The
//! field then renders as `Null`. The signal is WARN-level, so
//! `inv-no-observed-errors` (which keys on ERROR) never saw it, and the count
//! drifted from three to eight unnoticed. See the bugfunnel entry
//! `2026-08-24-declared-column-absent-narrow-subscription-projections`.
//!
//! A projection narrower than the entity schema is not by itself a gap: a
//! subscription whose renderer never reads the missing column, or reads it
//! under a widget parameter that declares a default, degrades as documented.
//! Only a REQUIRED column reaches this invariant.
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
/// Deliberately the EXTRACTED pair and not the raw warning text: a payload
/// carrying [`SIGNAL`] verbatim would be re-read as a gap on the next
/// transition — the self-feeding escape loop `test_tracing` records from
/// 2026-07-11. The pair form cannot match the filter.
#[holon_macros::capmap_adapter]
pub trait DeclaredColumnGaps {
    fn declared_column_gaps(&self) -> Vec<String>;
}

/// Marks a warning that carried [`SIGNAL`] but whose `context`/`column` fields
/// could not be read. Deliberately carries NO raw text: text carrying
/// [`SIGNAL`] would be re-read as a gap on the next transition.
const MALFORMED: &str = "MALFORMED/unparseable-gap-warning";

/// `context="x" column="y"` → `x/y`. The capture layer flattens an event's
/// fields into its message, so the pair is recovered from the text.
///
/// A SIGNAL-carrying warning whose fields do not parse means the capture format
/// moved under us. Dropping it would make this oracle green forever while
/// `n/n` still read engaged — silent vacuity — so it is surfaced twice: as an
/// ERROR (which `inv-no-observed-errors` fails on, carrying the raw text for
/// diagnosis) and as [`MALFORMED`] in the returned set, which fails this check.
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

/// Reports bindings that delivered rows short of the columns their renderer
/// requires.
pub struct InvNoDeclaredColumnAbsent;

impl InvNoDeclaredColumnAbsent {
    pub const ID: InvariantId = InvariantId("inv-no-declared-column-absent");

    pub fn new() -> Self {
        Self
    }
}

impl Default for InvNoDeclaredColumnAbsent {
    fn default() -> Self {
        Self::new()
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
        InvariantResult::Fail(format!(
            "[inv-no-declared-column-absent] {} projection gap(s): a binding delivered rows \
             short of a column its renderer REQUIRES, so those computed fields rendered as \
             Null. Either widen the offending SELECT, or — if the renderer can draw without \
             the column — bind it under a widget parameter that declares a default so the \
             degradation is documented:\n  {}",
            gaps.len(),
            gaps.join("\n  "),
        ))
    }
}
