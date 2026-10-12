//! The wire contract between a net executor and a viewer.
//!
//! Thin on purpose: the NET-rev1 executor is still moving, so this module
//! carries only the shapes the viewer already needs — structure, a marking
//! snapshot, a step delta and a why-not answer. It holds no `Instant`, no
//! closure and no executor type, which is what makes it serde- and
//! wasm-clean.
//!
//! Ids are the rendered forms their producers already publish: a place is
//! `"relation.field"`, a transition is a `holon_net::TransitionKey`
//! (`"op:block.set_field"`, `"rule:block:rule-daily-journal"`). The contract
//! does not re-type them, so it needs no dependency on `holon-net`.

use serde::Deserialize;
use serde::Serialize;

use crate::net_view::ArcKind;

/// A hash of the sources a structure was derived from. A client that sees a
/// `seq` gap, or a different `version`, fetches a fresh [`NetStructure`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NetVersion(String);

impl NetVersion {
    pub fn new(version: impl Into<String>) -> NetVersion {
        NetVersion(version.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How much of a token's value a place may show. The default is the safe end:
/// an undeclared place shows a digest, never a value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disclosure {
    Summary,
    #[default]
    Digest,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceDecl {
    pub id: String,
    pub label: String,
    pub token_type: Option<String>,
    pub subnet: Option<String>,
    #[serde(default)]
    pub disclosure: Disclosure,
}

/// What declared a transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Provenance {
    Operation {
        entity: String,
        op: String,
    },
    /// `active` is the rule watcher's own verdict: a parked rule is still a
    /// declared transition and still appears in the net.
    Rule {
        block_id: String,
        name: String,
        active: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardDecl {
    pub name: String,
    pub expr: Option<String>,
    /// The arc language cannot express this guard, so enabledness over it
    /// cannot be decided from the arcs alone.
    pub opaque: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionDecl {
    pub key: String,
    pub label: String,
    pub provenance: Provenance,
    pub guard: Option<GuardDecl>,
    pub subnet: Option<String>,
    /// `false` means "cannot say", never "touches nothing".
    pub analyzable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArcDecl {
    pub transition: String,
    pub place: String,
    pub kind: ArcKind,
    /// Which guard disjunct (DNF mode) this arc belongs to.
    pub mode: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubnetDecl {
    pub id: String,
    pub label: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetStructure {
    pub version: NetVersion,
    pub places: Vec<PlaceDecl>,
    pub transitions: Vec<TransitionDecl>,
    pub arcs: Vec<ArcDecl>,
    pub subnets: Vec<SubnetDecl>,
}

/// A token, as a viewer may see it: identity and a digest, never a value
/// unless the place's [`Disclosure`] allows a summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenView {
    pub uri: String,
    pub version: u64,
    pub digest: String,
    pub summary: Option<String>,
}

/// `tokens` is `None` until the client opens that place: a data place can
/// hold tens of thousands of rows, so a snapshot carries counts by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceMarking {
    pub place: String,
    pub count: u64,
    pub tokens: Option<Vec<TokenView>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkingSnapshot {
    pub version: NetVersion,
    /// The executor step this snapshot is consistent with.
    pub seq: u64,
    pub at_ms: i64,
    pub places: Vec<PlaceMarking>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum NetEvent {
    TokenPut {
        place: String,
        token: TokenView,
    },
    TokenTaken {
        place: String,
        uri: String,
        version: u64,
    },
    /// The client re-fetches the structure; its cached layout is stale.
    StructureChanged {
        new_version: NetVersion,
    },
}

/// One executor step that changed something. `seq` is per executor and
/// strictly increasing; a gap means the client resynchronises from a fresh
/// [`MarkingSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepDelta {
    pub version: NetVersion,
    pub seq: u64,
    pub at_ms: i64,
    pub events: Vec<NetEvent>,
}

/// Why a transition did not fire. Every failing condition, not the first one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// `kind`, not `reason`: the `Refused` variant already carries a `reason`
// field, and serde refuses a tag that collides with one.
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reason {
    MissingInput {
        place: String,
    },
    Inhibited {
        place: String,
        uri: String,
    },
    GuardFalse {
        guard: String,
        detail: Option<String>,
    },
    Running {
        attempt: u64,
        since_ms: i64,
    },
    Refused {
        class: String,
        reason: String,
    },
    /// The executor could not decide — production residue, not an empty
    /// answer.
    Unknown {
        why: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhyNot {
    pub transition: String,
    pub uri: String,
    pub seq: u64,
    pub enabled: bool,
    pub reasons: Vec<Reason>,
}

impl WhyNot {
    /// `enabled` is DERIVED from `reasons`, never passed: the executor's
    /// decision and its explanation come from one function, so "fires" means
    /// exactly "no reason was collected".
    pub fn new(
        transition: impl Into<String>,
        uri: impl Into<String>,
        seq: u64,
        reasons: Vec<Reason>,
    ) -> WhyNot {
        WhyNot {
            transition: transition.into(),
            uri: uri.into(),
            seq,
            enabled: reasons.is_empty(),
            reasons,
        }
    }
}
