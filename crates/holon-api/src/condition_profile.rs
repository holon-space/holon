//! What a condition IS, declared once per kind.
//!
//! [`ConditionBus`](crate::ConditionBus) carries an instance: a subject and the
//! payload of whatever went wrong. Everything a frontend needs in order to
//! DRAW that instance — how bad it is, where it belongs, when it goes away and
//! what the user may do about it — is a property of the kind, and lives here in
//! the one table [`ConditionKind::profile`].
//!
//! Before this table each of those four facts was re-invented per frontend: a
//! 22-arm colour match, a 300-line placement match and a mirrored kind enum in
//! the GPUI layer alone. A second frontend would have re-invented all three.
//!
//! Ratified in D121.a (2026-09-14); the reasoning is ADR 0035.

use std::time::Duration;

use crate::condition_bus::ConditionKind;

/// How loudly a condition is drawn. One value per kind, never per instance:
/// per-instance severity is what turned the frontend's colour lookup into a
/// 22-arm match in the first place (D121.a, question 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConditionSeverity {
    /// Not a degradation. Feedback about something that already resolved
    /// itself, and the only severity allowed to expire on a timer.
    Info,
    Warning,
    Error,
}

/// A Settings destination a remedy can open, and a surface a condition can be
/// placed in. Closed: a section that shows conditions has to be named here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    Integrations,
}

/// Where the condition is drawn. Declared here rather than chosen by each
/// frontend, so a second frontend inherits the boot banner instead of
/// re-deriving it (D121.a, question 4). A frontend with no surface for the
/// declared placement falls back to `Toast` and says so in its log; it never
/// drops the disclosure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConditionPlacement {
    Toast,
    Banner,
    Modal,
    Section(SettingsSection),
}

/// An operation a remedy can dispatch, as a closed vocabulary rather than an
/// op-name string, so an unknown op cannot be written into a profile.
///
/// One entry today, because one remedy button exists today. Inc 3 maps these
/// onto `OperationIntent` and grows the list as remedies are declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpId {
    PairRetryReimport,
}

impl OpId {
    pub const fn entity(self) -> &'static str {
        match self {
            Self::PairRetryReimport => "device",
        }
    }

    pub const fn op(self) -> &'static str {
        match self {
            Self::PairRetryReimport => "pair_retry_reimport",
        }
    }
}

/// Something the user may do about a condition. The kind decides which of
/// these are legal, so an illegal remedy is unrepresentable rather than
/// rejected at runtime.
///
/// `SetSetting` is deliberately absent: it needs `PrefKey`, which still lives
/// in `holon-frontend` and moves down in Inc 3. A stringly-typed placeholder
/// would defeat the point of this enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemedySlot {
    Retry(OpId),
    OpenSettings(SettingsSection),
    /// Reveal the path the instance carries. Which path is instance data, so
    /// the slot names no path of its own.
    RevealPath,
    Dismiss,
}

/// The internal success that ends a condition.
///
/// Not [`OpId`]: these are not dispatcher operations. A snapshot save, a SQL
/// projection and a rehydration are internal moments with no `OperationIntent`
/// behind them, so typing them as ops would claim a dispatch path that does
/// not exist. Inc 3 teaches the holder to observe these and clear on a subject
/// match; today each success site calls `clear` itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearingEvent {
    SnapshotSave,
    SqlProjection,
    ShareMaterialize,
    WritebackRespawn,
    IntegrationConnect,
    PairReimport,
}

/// The named moment that clears a condition.
///
/// Every kind names one. A kind that cannot name one does not belong on the
/// bus — that rule predates this type, but until now it was prose in a doc
/// comment and nothing could check it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllClear {
    NextSuccessOf(ClearingEvent),
    /// The next ingest of the file the instance names that reports no problem.
    NextCleanIngest,
    RemedyApplied,
    /// Info only. See [`ConditionProfile::new`].
    Elapsed(Duration),
    /// Nothing clears it in this process. Legal, and disclosed as such — a
    /// condition that is true for the whole session should say so rather than
    /// vanish and look resolved.
    UntilRestart,
}

/// The glyphs a condition can be drawn with.
///
/// Severity decides the COLOUR, but not the mark: five different Warnings wear
/// ⚠, ↻, 🔑, ↩ and 🎟, and which one is a fact about the kind, exactly like its
/// label. Named constants rather than inline literals so the frontend's
/// icon-font coverage test can sweep [`CONDITION_ICONS`] instead of a
/// hand-maintained list that drifts when a profile is added.
pub mod icons {
    /// The general degradation mark. Most Warnings, and the Errors that are
    /// not refusals.
    pub const WARN: &str = "⚠";
    /// A refusal: the operation was blocked, nothing changed.
    pub const BLOCKED: &str = "⛔";
    /// A retry is what recovers this.
    pub const RETRY: &str = "↻";
    /// About a credential or a key.
    pub const KEY: &str = "🔑";
    /// About undo history.
    pub const UNDO: &str = "↩";
    /// About a share ticket.
    pub const TICKET: &str = "🎟";
    /// Not a degradation — feedback.
    pub const INFO: &str = "i";
}

/// Every distinct glyph a profile can carry. The frontend's icon-font coverage
/// test sweeps this, so a new profile's glyph is checked for renderability
/// without anyone remembering to add it anywhere.
pub const CONDITION_ICONS: &[&str] = &[
    icons::WARN,
    icons::BLOCKED,
    icons::RETRY,
    icons::KEY,
    icons::UNDO,
    icons::TICKET,
    icons::INFO,
];

/// Everything about a condition that does not vary by instance.
///
/// The fields are private so that [`ConditionProfile::new`] really is the only
/// way in: the two combinations it refuses cannot be written past with a struct
/// literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConditionProfile {
    severity: ConditionSeverity,
    /// The fixed headline. Instance detail is carried by the condition, never
    /// by this string.
    label: &'static str,
    /// The mark drawn beside the label, from [`icons`]. Not derivable from
    /// severity — see that module.
    icon: &'static str,
    placement: ConditionPlacement,
    all_clear: AllClear,
    remedies: &'static [RemedySlot],
}

impl ConditionProfile {
    pub const fn severity(self) -> ConditionSeverity {
        self.severity
    }

    pub const fn label(self) -> &'static str {
        self.label
    }

    pub const fn icon(self) -> &'static str {
        self.icon
    }

    pub const fn placement(self) -> ConditionPlacement {
        self.placement
    }

    pub const fn all_clear(self) -> AllClear {
        self.all_clear
    }

    pub const fn remedies(self) -> &'static [RemedySlot] {
        self.remedies
    }

    /// The only constructor, and it refuses two combinations outright.
    ///
    /// Both rules protect the same thing: a condition that stops being shown
    /// while it is still true. `Elapsed` hides a live degradation on a timer,
    /// and `Dismiss` hides one on a click — acceptable for `Info`, which is
    /// feedback rather than degradation, and never for `Warning` or `Error`
    /// (D121.a, questions 6 and 7).
    ///
    /// Every profile below is a `const`, so a violation is a compile error
    /// rather than a panic the first time that condition is raised.
    pub const fn new(
        severity: ConditionSeverity,
        label: &'static str,
        icon: &'static str,
        placement: ConditionPlacement,
        all_clear: AllClear,
        remedies: &'static [RemedySlot],
    ) -> Self {
        let expires_on_its_own = matches!(all_clear, AllClear::Elapsed(_));
        if expires_on_its_own && !matches!(severity, ConditionSeverity::Info) {
            panic!(
                "AllClear::Elapsed is legal only for ConditionSeverity::Info: a Warning or Error \
                 that vanishes on a timer is a degradation the user stops being told about"
            );
        }
        let mut i = 0;
        while i < remedies.len() {
            if matches!(&remedies[i], RemedySlot::Dismiss)
                && !matches!(all_clear, AllClear::RemedyApplied)
                && !expires_on_its_own
            {
                panic!(
                    "RemedySlot::Dismiss is legal only where AllClear is RemedyApplied or \
                     Elapsed: dismissing a condition that is still true hides it instead \
                     of resolving it"
                );
            }
            i += 1;
        }
        Self {
            severity,
            label,
            icon,
            placement,
            all_clear,
            remedies,
        }
    }
}

// One profile per kind. Each `label` and `severity` is the value the GPUI
// frontend paints today, so Inc 2 can delete its colour match without changing
// a pixel; each `all_clear` is the moment that variant's doc comment already
// promised on `ConditionKind`.

const SNAPSHOT_SAVE_FAILED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Snapshot save failed",
    icons::WARN,
    ConditionPlacement::Toast,
    AllClear::NextSuccessOf(ClearingEvent::SnapshotSave),
    &[],
);

const SNAPSHOT_LOAD_FAILED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Error,
    "Shared snapshot could not be read",
    icons::BLOCKED,
    ConditionPlacement::Modal,
    AllClear::UntilRestart,
    &[],
);

const REHYDRATION_FAILED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Rehydration failed",
    icons::RETRY,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

const SQL_PROJECTION_FAILED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Shared edit not shown",
    icons::WARN,
    ConditionPlacement::Toast,
    AllClear::NextSuccessOf(ClearingEvent::SqlProjection),
    &[],
);

const FOREIGN_ID_COLLISION: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Error,
    "Blocked shared write (id collision)",
    icons::BLOCKED,
    ConditionPlacement::Toast,
    AllClear::NextSuccessOf(ClearingEvent::SqlProjection),
    &[],
);

const VAULT_INGEST_FAILED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Error,
    "File sync degraded (bad vault file)",
    icons::WARN,
    ConditionPlacement::Toast,
    AllClear::NextCleanIngest,
    &[],
);

const VAULT_FILE_EMPTIED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Vault file is empty — the document shown is stale",
    icons::WARN,
    ConditionPlacement::Toast,
    AllClear::NextCleanIngest,
    &[],
);

const SHARED_SUBTREE_NOT_MATERIALIZED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Shared subtree not materialized",
    icons::WARN,
    ConditionPlacement::Toast,
    AllClear::NextSuccessOf(ClearingEvent::ShareMaterialize),
    &[],
);

const EDIT_REFUSED_READ_ONLY_FORMAT: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Error,
    "Edit refused — read-only file",
    icons::BLOCKED,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

const LOCAL_EDIT_NOT_APPLIED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Error,
    "Your edit was not saved",
    icons::BLOCKED,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

const WRITEBACK_DEGRADED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Error,
    "Edits are not reaching disk",
    icons::BLOCKED,
    ConditionPlacement::Toast,
    AllClear::NextSuccessOf(ClearingEvent::WritebackRespawn),
    &[],
);

const INTEGRATION_CONNECT_FAILED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Error,
    "Integration unavailable",
    icons::BLOCKED,
    ConditionPlacement::Toast,
    AllClear::NextSuccessOf(ClearingEvent::IntegrationConnect),
    &[],
);

const INTEGRATION_NEEDS_AUTH: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Error,
    "Integration needs authorization",
    icons::BLOCKED,
    ConditionPlacement::Toast,
    AllClear::NextSuccessOf(ClearingEvent::IntegrationConnect),
    &[],
);

const INTEGRATION_SIDECAR_SUPERSEDED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Installed integration file ignored — using the bundled one",
    icons::WARN,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

const INTEGRATION_NOT_ENABLED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Integration is not switched on",
    icons::WARN,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

const INTEGRATION_SIDECAR_NOT_BUNDLED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "No connection by this name",
    icons::WARN,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

const INTEGRATION_SIDECAR_UNUSABLE: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Connection file cannot be used",
    icons::WARN,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

const SECRETS_HELD_IN_MEMORY: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Secrets are not being saved",
    icons::KEY,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

const UNDO_HISTORY_CLEARED_AT_BOOT: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Undo history did not survive the restart",
    icons::UNDO,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

const PAIRING_REIMPORTED_LOCAL_CONTENT: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Info,
    "Content kept from this device",
    icons::INFO,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

/// The one condition that offers the user an action today. Its banner's Retry
/// button is hand-wired in the GPUI layer; Inc 3 renders it from this slot and
/// deletes that wiring.
const PAIRING_REIMPORT_DEFERRED: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Some blocks could not be re-imported",
    icons::WARN,
    ConditionPlacement::Banner,
    AllClear::NextSuccessOf(ClearingEvent::PairReimport),
    &[RemedySlot::Retry(OpId::PairRetryReimport)],
);

const BEARER_TICKET_ENROLLMENT: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Peer joined with a share ticket",
    icons::TICKET,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

const OWNER_RECOVERY_CODE_NOT_SHOWN: ConditionProfile = ConditionProfile::new(
    ConditionSeverity::Warning,
    "Sharing key has no recovery code",
    icons::KEY,
    ConditionPlacement::Toast,
    AllClear::UntilRestart,
    &[],
);

impl ConditionKind {
    /// This kind's profile. Total, like
    /// [`condition_kind`](ConditionKind::condition_kind): a new variant cannot
    /// compile until it declares how it is drawn and what ends it.
    pub const fn profile(&self) -> ConditionProfile {
        match self {
            Self::SnapshotSaveFailed(_) => SNAPSHOT_SAVE_FAILED,
            Self::SnapshotLoadFailed(_) => SNAPSHOT_LOAD_FAILED,
            Self::RehydrationFailed(_) => REHYDRATION_FAILED,
            Self::SqlProjectionFailed(_) => SQL_PROJECTION_FAILED,
            Self::ForeignIdCollision(_) => FOREIGN_ID_COLLISION,
            Self::VaultIngestFailed { .. } => VAULT_INGEST_FAILED,
            Self::VaultFileEmptied => VAULT_FILE_EMPTIED,
            Self::SharedSubtreeNotMaterialized { .. } => SHARED_SUBTREE_NOT_MATERIALIZED,
            Self::EditRefusedReadOnlyFormat { .. } => EDIT_REFUSED_READ_ONLY_FORMAT,
            Self::LocalEditNotApplied { .. } => LOCAL_EDIT_NOT_APPLIED,
            Self::WritebackDegraded(_) => WRITEBACK_DEGRADED,
            Self::IntegrationConnectFailed { .. } => INTEGRATION_CONNECT_FAILED,
            Self::IntegrationNeedsAuth { .. } => INTEGRATION_NEEDS_AUTH,
            Self::IntegrationSidecarSuperseded { .. } => INTEGRATION_SIDECAR_SUPERSEDED,
            Self::IntegrationNotEnabled { .. } => INTEGRATION_NOT_ENABLED,
            Self::IntegrationSidecarNotBundled { .. } => INTEGRATION_SIDECAR_NOT_BUNDLED,
            Self::IntegrationSidecarUnusable { .. } => INTEGRATION_SIDECAR_UNUSABLE,
            Self::SecretsHeldInMemory { .. } => SECRETS_HELD_IN_MEMORY,
            Self::UndoHistoryClearedAtBoot { .. } => UNDO_HISTORY_CLEARED_AT_BOOT,
            Self::PairingReimportedLocalContent { .. } => PAIRING_REIMPORTED_LOCAL_CONTENT,
            Self::PairingReimportDeferred { .. } => PAIRING_REIMPORT_DEFERRED,
            Self::BearerTicketEnrollment { .. } => BEARER_TICKET_ENROLLMENT,
            Self::OwnerRecoveryCodeNotShown => OWNER_RECOVERY_CODE_NOT_SHOWN,
        }
    }
}
