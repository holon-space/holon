//! What a condition INSTANCE says, rendered from its payload.
//!
//! [`ConditionProfile`](crate::condition_profile::ConditionProfile) is the
//! kind-level table — severity, label, icon, placement. This is its
//! instance-level counterpart: the sentence that names the file, the peer, the
//! integration or the count this particular raise carries.
//!
//! It lives here rather than in a frontend because it was the last thing left
//! in the GPUI layer's 320-line per-kind match, and a second frontend would
//! have had to write all of it again — differently. It is also what lets the
//! `conditions` row source grow a `detail` column without minting a second,
//! divergent copy of the same prose.
//!
//! **Headline and body are not interchangeable.** The toast render caps the
//! headline, so anything the user must reproduce CHARACTER-EXACT — a path, a
//! command to run, a URL to open, the query that finds the conflict copies —
//! belongs in `body`, where it is never truncated. Several of these were once
//! one long headline and the cap ate the actionable half.

use crate::condition_bus::ConditionKind;

/// One condition instance's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConditionDetail {
    /// The sentence. Subject to the render's length cap.
    pub headline: String,
    /// Payload lines shown in full, one per entry. Never capped.
    pub body: Vec<String>,
}

impl ConditionDetail {
    pub fn prose(headline: impl Into<String>) -> Self {
        Self {
            headline: headline.into(),
            body: Vec::new(),
        }
    }

    pub fn with_body(headline: impl Into<String>, body: Vec<String>) -> Self {
        Self {
            headline: headline.into(),
            body,
        }
    }
}

impl ConditionKind {
    /// What this instance says, given the subject it was raised against.
    ///
    /// Total, like [`condition_kind`](ConditionKind::condition_kind) and
    /// [`profile`](crate::condition_profile::ConditionProfile): a new variant
    /// cannot compile until it says what it tells the user.
    pub fn detail(&self, subject: &str) -> ConditionDetail {
        match self {
            Self::SnapshotSaveFailed(detail)
            | Self::SnapshotLoadFailed(detail)
            | Self::RehydrationFailed(detail)
            | Self::SqlProjectionFailed(detail)
            | Self::ForeignIdCollision(detail)
            | Self::WritebackDegraded(detail) => ConditionDetail::prose(detail.clone()),

            // The file, so the headline can name it and the condition clears
            // per file rather than per scan.
            Self::VaultIngestFailed { reason, .. } => {
                ConditionDetail::prose(format!("{subject}: {reason}"))
            }

            Self::VaultFileEmptied => ConditionDetail::prose(format!(
                "{subject} is empty on disk — Holon kept the document it last read from it, so \
                 what you see is no longer in the file"
            )),

            // The file the shared content was inlined into — what the user
            // opens to see the stale projection.
            Self::SharedSubtreeNotMaterialized { file } => ConditionDetail::prose(file.clone()),

            Self::LocalEditNotApplied { detail } => ConditionDetail::prose(format!(
                "what you typed into {subject} is not in the store — retype it. {detail}"
            )),

            Self::EditRefusedReadOnlyFormat { format } => ConditionDetail::prose(format!(
                "{subject} is {format}, which Holon reads but cannot write — edit the file on disk"
            )),

            // The archive path is a BODY line: the user reproduces it
            // character for character, and the headline is capped.
            Self::PairingReimportedLocalContent {
                blocks,
                conflict_copies,
                archive,
            } => ConditionDetail::with_body(
                format!(
                    "{blocks} block(s) written on this device were added to the paired store, \
                     {conflict_copies} of them kept as a copy under the owner's block of the same \
                     id. The pre-pair document is here:"
                ),
                vec![archive.clone()],
            ),

            Self::PairingReimportDeferred { orphans, archive } => ConditionDetail::with_body(
                format!(
                    "{orphans} block(s) could not be re-imported into the paired store. They are \
                     kept here:"
                ),
                vec![archive.clone()],
            ),

            // The toast body truncates the headline, so this leads with the
            // integration name.
            Self::IntegrationConnectFailed { integration, error } => {
                ConditionDetail::prose(format!("{integration}: {error}"))
            }

            Self::IntegrationNeedsAuth {
                integration,
                auth_url,
            } => ConditionDetail::with_body(
                format!("{integration} needs authorizing. Open:"),
                vec![auth_url.clone()],
            ),

            Self::IntegrationSidecarSuperseded {
                integration,
                installed_path,
                bundled_source,
                incompatibility,
            } => ConditionDetail::with_body(
                format!(
                    "{integration}: the installed file was ignored ({incompatibility}); the \
                     bundled {bundled_source} is running instead. The ignored file:"
                ),
                vec![installed_path.clone()],
            ),

            // Headline, then the two payloads that must reach the user
            // CHARACTER-EXACT and so never see the cap: the command to run (a
            // remedy cut in half reads as complete and does not work) and the
            // file it writes. The cap ate both when all three shared one
            // string.
            Self::IntegrationNotEnabled {
                integration,
                state_path,
                remedy,
                ..
            } => ConditionDetail::with_body(
                format!(
                    "{integration} is installed but switched off, so it runs nothing. Switch it \
                     on in Settings › Integrations, or run:"
                ),
                vec![remedy.clone(), state_path.clone()],
            ),

            Self::IntegrationSidecarNotBundled {
                provider,
                installed_path,
            } => ConditionDetail::with_body(
                format!(
                    "{provider}: nothing provides a connection by this name, so this file runs \
                     nothing:"
                ),
                vec![installed_path.clone()],
            ),

            // The file and the REASON are both body lines. The reason ends in
            // the remedy — the clause saying what to change — and it was the
            // half the cap ate.
            Self::IntegrationSidecarUnusable {
                provider,
                installed_path,
                why,
            } => ConditionDetail::with_body(
                format!("{provider}: this connection file cannot be used."),
                vec![installed_path.clone(), why.clone()],
            ),

            // The seed file is a PAYLOAD, not prose: this banner is what stops a
            // screenshot passing a fixture off as a real credential, so it owes
            // the reader the file NAME, and the name sits at the tail of a long
            // temp path the headline cap would eat.
            Self::SecretsHeldInMemory { why, seed_path } => {
                ConditionDetail::with_body(why.clone(), seed_path.iter().cloned().collect())
            }

            Self::UndoHistoryClearedAtBoot { entries } => ConditionDetail::prose(format!(
                "{entries} step{} from the previous session were discarded",
                if *entries == 1 { "" } else { "s" }
            )),

            Self::BearerTicketEnrollment { peer } => ConditionDetail::prose(format!(
                "peer {peer} joined by presenting the share ticket. Anyone the ticket was \
                 forwarded to could have joined instead — unshare, or revoke the peer, if that \
                 was not intended"
            )),

            // Says what is and is not at risk, because "no recovery code"
            // reads as "my data is one keychain away from gone" and that is
            // not what happened.
            Self::OwnerRecoveryCodeNotShown => ConditionDetail::prose(
                "this device made its sharing identity key on the first share, and its one-time \
                 recovery code could not be shown. Shared content and peer access are unaffected; \
                 if the keychain entry is lost, this device's shares stop being advertised until \
                 it is shared again",
            ),
        }
    }
}
