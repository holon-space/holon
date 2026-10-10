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
            | Self::WritebackDegraded(detail)
            | Self::WritebackLossy { detail } => ConditionDetail::prose(detail.clone()),

            // The capped headline names one file or counts them; each example's
            // path and reason, an error chain whose cause comes last, go whole.
            Self::VaultIngestFailed(refusals) => {
                let count = refusals.count().get();
                let examples = refusals.examples();
                let headline = if count == 1 {
                    let path = std::path::Path::new(&examples[0].path);
                    let name = path
                        .file_name()
                        .map_or(path.to_string_lossy(), |name| name.to_string_lossy());
                    format!("{name} was not read into Holon. The file and why:")
                } else {
                    format!(
                        "{count} {} files were not read into Holon. The first {} and why:",
                        refusals.format(),
                        examples.len()
                    )
                };
                let mut body: Vec<String> = examples
                    .iter()
                    .flat_map(|file| [file.path.clone(), file.reason.clone()])
                    .collect();
                if count > examples.len() {
                    body.push(format!(
                        "and {} more {} files",
                        count - examples.len(),
                        refusals.format()
                    ));
                }
                ConditionDetail::with_body(headline, body)
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

            Self::EditRefusedByShape { tag, rule } => ConditionDetail::prose(format!(
                "the edit would leave the {tag} block {subject} breaking rule {rule}, so it was \
                 not made"
            )),

            Self::EditRefusedReadOnlyFormat { format } => ConditionDetail::prose(format!(
                "{subject} is {format}, which Holon reads but cannot write — edit the file on disk"
            )),

            Self::LeftSharedPage { title } => ConditionDetail::prose(format!(
                "You left the shared page {title:?}; the owner's page is unchanged"
            )),

            Self::DeletedSharedPage { title } => ConditionDetail::prose(format!(
                "You deleted {title:?}, which you shared; the share is revoked and its recipients \
                 lose it"
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

            Self::IntegrationConnectSlow {
                integration,
                elapsed_secs,
            } => ConditionDetail::prose(format!(
                "{integration} has been connecting for {elapsed_secs} s; its pages fill in when it \
                 answers"
            )),

            Self::IntegrationWaitingOnKeychain { integration } => ConditionDetail::prose(format!(
                "{integration} is waiting to read its credentials from the keychain; answer the \
                 keychain prompt to let it connect"
            )),

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

            Self::DatabaseRebuiltAtBoot {
                reason,
                lost,
                caches,
            } => ConditionDetail::with_body(
                format!(
                    "The database was rebuilt because {reason}. Blocks and views came back from \
                     the vault; the integration caches re-sync from their sources, without the \
                     rows a source no longer holds; this local state did not come back"
                ),
                lost.iter()
                    .map(|l| format!("{}: {} ({} rows)", l.table, l.what, l.rows))
                    .chain(caches.iter().map(|c| {
                        format!(
                            "{}: {} cache, re-syncing ({} rows)",
                            c.table, c.provider, c.rows
                        )
                    }))
                    .collect(),
            ),

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

            Self::NestedShareLoaded { mount } => ConditionDetail::prose(format!(
                "shared tree {subject} contains another share (mount {mount}). Holon does not \
                 support a share inside a share, so pages in it may not resolve. To fix it, \
                 stop sharing {subject} and share its parts one by one"
            )),

            // Block ids are BODY lines: the user finds and deletes them by id.
            // The remedy names no mount to delete, so it stays right while the
            // user works through the list.
            Self::DuplicateMount {
                canonical,
                duplicates,
            } => ConditionDetail::with_body(
                format!(
                    "shared tree {subject} is mounted more than once, because two of your devices \
                     accepted it before they synced. Holon shows it at {canonical}. To fix it, \
                     keep one of these mounts and delete the others as blocks; unsharing one \
                     stops the share for all of them:"
                ),
                std::iter::once(canonical.clone())
                    .chain(duplicates.iter().cloned())
                    .collect(),
            ),

            Self::VaultBacklogIngesting { done, total } => ConditionDetail::with_body(
                format!(
                    "Holon syncs the files in {subject}; {done} of its {total} read-only files \
                     are read so far, and the rest appear as they are read."
                ),
                vec![],
            ),

            Self::WatchViewsRebuilding => ConditionDetail::prose(
                "every watched view is being dropped and recreated; lists may lag until it \
                 finishes",
            ),

            Self::ProfileRefused { error } => ConditionDetail::with_body(
                format!("the entity profile in {subject} is not applied — fix it:"),
                vec![error.clone()],
            ),

            // The paths are BODY lines: the user opens one of them to delete a
            // copy, so they must survive the headline cap.
            Self::BlockInTwoFiles {
                owner_file,
                copy_files,
            } => ConditionDetail::with_body(
                format!(
                    "{subject} is in {} files. The first stays authoritative; delete the copy \
                     you do not want:",
                    1 + copy_files.len()
                ),
                std::iter::once(owner_file.clone())
                    .chain(copy_files.iter().cloned())
                    .collect(),
            ),

            Self::BlockEditedInTwoFiles {
                owner_file,
                copy_files,
            } => ConditionDetail::with_body(
                format!(
                    "{subject} was edited in Holon and in a copy in another file. Both are kept; \
                     delete the one you do not want:",
                ),
                std::iter::once(owner_file.clone())
                    .chain(copy_files.iter().cloned())
                    .collect(),
            ),

            Self::DeletedBlockKeptInFile { file } => ConditionDetail::with_body(
                format!("{subject} was deleted in Holon, but a copy of it is still in:"),
                vec![file.clone()],
            ),

            Self::DeletionUndoneBlockInOtherFile { file, copy_files } => {
                ConditionDetail::with_body(
                    format!(
                        "{subject} was deleted from {file}, and Holon put it back because the \
                         files listed hold a copy of it. Delete it from them too, and the \
                         deletion stands:"
                    ),
                    copy_files.clone(),
                )
            }

            Self::DeletionEndedByEdit { file } => ConditionDetail::with_body(
                format!(
                    "{subject} was deleted from {file}, and Holon put it back because another \
                     file held a copy of it. It was edited since, so the deletion no longer \
                     stands and the block stays. Delete it again where you want it gone."
                ),
                vec![file.clone()],
            ),

            Self::FileEditOverruled {
                file,
                file_text,
                change,
            } => ConditionDetail::with_body(
                format!(
                    "{subject} was {change} in Holon, and {file} was edited before Holon wrote \
                     that back. Holon's change stands; the file's text was not applied and is \
                     replaced on the next write. The text was:"
                ),
                vec![file_text.clone()],
            ),

            Self::VaultSyncNotStarted { cause } => ConditionDetail::with_body(
                format!(
                    "Holon is not syncing the files in {subject}: {cause}. Restart Holon after \
                     fixing the cause."
                ),
                vec![],
            ),

            Self::VaultStateUnreadable { kept_as, reason } => ConditionDetail::with_body(
                format!(
                    "{subject} could not be read ({reason}). Holon kept it as it was at \
                     {kept_as} and started without what it could not read. It lists deletions \
                     Holon undid because another file held a copy; those lines are back in \
                     their files. Delete them again where you want them gone."
                ),
                vec![kept_as.clone()],
            ),

            Self::VaultStartIncomplete { step, cause } => ConditionDetail::with_body(
                format!(
                    "Holon syncs the files in {subject}, but {step} failed at start: {cause}. \
                     Restart Holon after fixing the cause."
                ),
                vec![],
            ),

            Self::WrittenFilesUnrecorded { files, cause } => ConditionDetail::with_body(
                format!(
                    "Holon wrote these files in {subject} but could not record what it wrote \
                     ({cause}); the next start reads them again."
                ),
                files.clone(),
            ),

            Self::LoroUpdateLogTailDropped { bytes, reason } => ConditionDetail::prose(format!(
                "Holon dropped the last {bytes} bytes of {subject} at start, a save a crash cut \
                 short ({reason}). Edits made just before the crash may be missing."
            )),

            Self::ViewEngineStopped(reason) => ConditionDetail::prose(format!(
                "Holon's view engine stopped: {reason}. Its views keep the state before the \
                 error until Holon restarts."
            )),

            Self::OperationCatalogStopped(reason) => ConditionDetail::prose(format!(
                "Rows stopped following Holon's operation catalog: {reason}. An operation \
                 registered from now on can be run but appears on no row until Holon restarts."
            )),

            Self::DatabaseStuck {
                command,
                running_secs,
                ..
            } => ConditionDetail::prose(format!(
                "Holon's database has run one {command} command for {running_secs} s without \
                 finishing. Every read and write waits behind it. The log has the full report."
            )),

            Self::DatabaseWatchFailed { cause } => {
                ConditionDetail::prose(format!("Holon's watch over its database failed: {cause}."))
            }

            Self::DerivedFieldNotComputed {
                block_id,
                field,
                reason,
            } => ConditionDetail::prose(format!(
                "Holon could not compute {field} of {block_id}: {reason}. It shows no value \
                 until the next computation succeeds."
            )),

            Self::BlockFieldUnreadable {
                block_id,
                field,
                stored,
                reason,
            } => ConditionDetail::prose(format!(
                "{block_id} stores {field} as {stored}, which Holon cannot read ({reason}), so it \
                 shows the default {field}. Write a valid {field} with set_field to fix it."
            )),

            // A doorbell: every record is read in full in the crash history,
            // which the profile's remedy opens.
            Self::PreviousRunPanicked {
                message, earlier, ..
            } => {
                let runs = match 1 + earlier.runs() {
                    1 => "1 earlier run".to_string(),
                    runs => format!("{runs} earlier runs"),
                };
                ConditionDetail::prose(format!(
                    "Holon crashed in {runs}; the newest at {subject}: {}",
                    one_line(message)
                ))
            }

            Self::TaskPanicked { message, thread } => ConditionDetail::with_body(
                format!(
                    "An internal error stopped {thread}; what it was doing does not continue \
                     until Holon restarts."
                ),
                vec![subject.to_string(), message.clone()],
            ),

            Self::PanicRecordUnreadable { reason } => ConditionDetail::prose(format!(
                "Holon cannot read {subject} among its records of internal errors: {reason}. \
                 It stays where it is, and the records beside it are still shown."
            )),

            Self::PanicRecordUnwritable { reason } => ConditionDetail::prose(format!(
                "Holon cannot keep a record of internal errors in {subject}: {reason}. If it \
                 stops because of one, the next start does not say why."
            )),

            Self::PanicConditionsUnavailable { reason } => ConditionDetail::prose(format!(
                "Holon cannot show an internal error while it runs: {reason}. The next start \
                 shows the last one."
            )),

            Self::TypeTableRefused {
                table,
                why,
                quarantined,
            } => {
                let mut body = vec![why.clone()];
                body.extend(
                    quarantined.iter().map(|q| {
                        format!("Rows are kept, unread, in {} ({} rows).", q.name, q.rows)
                    }),
                );
                if quarantined.is_empty() {
                    body.push(format!(
                        "Remedy: forget Holon's record and serve {subject} from a new empty \
                         {table}."
                    ));
                } else {
                    let names: Vec<&str> = quarantined.iter().map(|q| q.name.as_str()).collect();
                    let rows: u64 = quarantined.iter().map(|q| q.rows).sum();
                    body.push(format!(
                        "Remedy: drop {}, {rows} rows lost, and serve {subject} from a new empty \
                         {table}. To keep rows, copy them out with SQL first.",
                        names.join(", ")
                    ));
                }
                ConditionDetail::with_body(
                    format!(
                        "{subject} is not available: Holon does not serve its stored table \
                         {table}, and no row was changed."
                    ),
                    body,
                )
            }

            Self::TableColumnsAdded {
                table,
                added,
                undeclared,
                rows,
            } => ConditionDetail::with_body(
                format!(
                    "{table} of {subject} gained the declared columns {}; its {rows} rows are kept.",
                    added.join(", ")
                ),
                undeclared
                    .iter()
                    .map(|c| format!("kept undeclared column {c}"))
                    .collect(),
            ),

            Self::TableRebuilt { diff, rows } => ConditionDetail::with_body(
                format!(
                    "{subject} did not match its declaration, so it was recreated empty and \
                     refilled from the org files and the Loro store; its {rows} stored rows \
                     were dropped."
                ),
                vec![diff.clone()],
            ),

            Self::SchemaModuleFailed { error } => ConditionDetail::with_body(
                format!(
                    "the {subject} tables could not be set up past their first failing \
                     statement, and none after it ran."
                ),
                vec![error.clone()],
            ),
        }
    }
}

/// How many characters of a panic message one line shows.
const ONE_LINE_CHARS: usize = 120;

/// The first line of `message`, cut to [`ONE_LINE_CHARS`]; `…` marks text
/// left out.
fn one_line(message: &str) -> String {
    let mut lines = message.lines();
    let first = lines.next().unwrap_or("");
    let cut: String = first.chars().take(ONE_LINE_CHARS).collect();
    if cut.len() < first.len() || lines.any(|l| !l.trim().is_empty()) {
        format!("{cut}…")
    } else {
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user deletes a copy by hand, so every file stays in the body.
    #[test]
    fn a_block_in_two_files_names_every_file() {
        let detail = ConditionKind::BlockInTwoFiles {
            owner_file: "/vault/DayPage.org".into(),
            copy_files: vec!["/vault/Overview.org".into()],
        }
        .detail("block:bulk-0-0");
        assert!(
            detail.body.iter().any(|l| l.ends_with("Overview.org"))
                && detail.body.iter().any(|l| l.ends_with("DayPage.org")),
            "every file stays in the body: {detail:?}"
        );
    }

    fn previous_run(message: &str) -> ConditionDetail {
        ConditionKind::PreviousRunPanicked {
            message: message.to_string(),
            thread: "main".to_string(),
            earlier: crate::EarlierPanics {
                panics: Vec::new(),
                dropped: None,
            },
        }
        .detail("boot.rs:1:1")
    }

    #[test]
    fn a_line_break_after_the_only_line_cuts_nothing() {
        for message in ["short message\n", "short message\r\n"] {
            let headline = previous_run(message).headline;
            assert!(!headline.contains('…'), "{message:?} -> {headline}");
        }
        let headline = previous_run("first line\nsecond line").headline;
        assert!(headline.ends_with("first line…"), "{headline}");
    }

    /// The doorbell is one line plus a way in: the records themselves are
    /// read in the crash history.
    #[test]
    fn the_previous_run_doorbell_is_its_headline_and_opens_the_crash_history() {
        let kind = ConditionKind::PreviousRunPanicked {
            message: "run 2 broke".to_string(),
            thread: "main".to_string(),
            earlier: crate::EarlierPanics {
                panics: vec![crate::EarlierPanic {
                    location: "a.rs:1:1".to_string(),
                    message: "run 1 broke".to_string(),
                }],
                dropped: None,
            },
        };
        let detail = kind.detail("b.rs:2:2");
        assert!(
            detail.body.is_empty(),
            "the doorbell has no body: {detail:?}"
        );
        assert!(
            detail.headline.contains("b.rs:2:2") && detail.headline.contains("run 2 broke"),
            "the headline names the newest site and message: {}",
            detail.headline
        );
        assert!(
            kind.profile()
                .remedies()
                .contains(&crate::RemedySlot::OpenSettings(
                    crate::SettingsSection::CrashHistory
                )),
            "the doorbell opens the crash history: {:?}",
            kind.profile().remedies()
        );
    }

    /// A window that cuts the headline short cuts its end, so the count and
    /// the site come before the message.
    #[test]
    fn the_previous_run_headline_names_the_site_before_the_message() {
        let headline = previous_run("called `Result::unwrap()` on an `Err` value").headline;
        let site = headline.find("boot.rs:1:1").expect("the site is named");
        let message = headline.find("called").expect("the message is named");
        assert!(site < message, "{headline}");
    }
}
