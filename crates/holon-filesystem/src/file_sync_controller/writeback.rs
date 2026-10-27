//! The effects of the per-file write-back net (ADR 0032 §9.5,
//! [`crate::writeback_net`]). The net owns each document's write-back state;
//! this module reads the disk, ingests and writes for it, and keeps the
//! conditions it shows in step with its marking.
//!
//! The authority and the write are not atomic (Q3.a): a block change that
//! reaches the store between the write and the `Base` record settles that
//! firing over changed inputs, and the newer `Authority` makes the file
//! `Behind` again, so the next `CatchUpWrite` writes it.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use holon_api::ConditionKind;
use holon_api::EntityUri;
use holon_core::CanonicalPath;
use holon_net::exec::Cause;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::Output;
use holon_net::exec::Subject;
use holon_net::exec::WhyNot;
use tracing::info;
use tracing::warn;

use super::FileSyncController;
use super::IngestOutcome;
use super::PathIntent;
use super::ProjectionWrite;
use super::QuarantineCause;
use super::read_basis;
use crate::base_sidecar::BaseSidecars;
use crate::writeback_net as wn;
use crate::writeback_net::field;
use crate::writeback_net::output;

/// A render of the store the next `CatchUpWrite` writes to `home` instead of
/// rendering again.
#[derive(Debug)]
pub(super) struct Owed {
    pub(super) rendered: String,
    pub(super) images: bool,
    pub(super) home: CanonicalPath,
}

fn hash_of(text: Option<&str>) -> String {
    text.map_or_else(|| wn::ABSENT.to_string(), BaseSidecars::hash)
}

fn doc_of(subject: &Subject) -> EntityUri {
    EntityUri::parse(subject.as_str()).expect("a write-back net subject is a document URI")
}

fn base(doc: &Subject, hash: &str, authority: &str) -> Output {
    output(wn::BASE, doc, [("hash", hash), ("authority", authority)])
}

fn on_disk(doc: &Subject, hash: &str) -> Output {
    output(wn::ON_DISK, doc, [("hash", hash)])
}

fn disk_changed(doc: &Subject) -> Output {
    output(wn::DISK_CHANGED, doc, [])
}

impl FileSyncController {
    fn net_token(&self, place: &str, doc: &EntityUri, name: &str) -> Option<String> {
        self.net.read(|exec| {
            exec.tokens()
                .token(place, doc.as_str())
                .map(|t| field(&t.content, name).to_string())
        })
    }

    /// The file the net reads for `doc`: its recorded home, else the path its
    /// name chain derives.
    async fn net_home(&self, doc: &EntityUri) -> Result<Option<CanonicalPath>> {
        if let Some(home) = self.doc_home.get(doc) {
            return Ok(Some(home.clone()));
        }
        Ok(self
            .doc_id_to_path(doc, PathIntent::WriteOwnFile)
            .await?
            .map(|p| CanonicalPath::new(p.as_path())))
    }

    /// Records the text Holon holds for `canonical` as the `Base` of `doc`.
    /// A document the net has not seen yet also gets a disk read, which is
    /// what makes a boot read every file once.
    pub(super) fn record_base(&mut self, doc: &EntityUri, canonical: &CanonicalPath) {
        let hash = hash_of(self.last_projection.get(canonical).map(String::as_str));
        let held = self.net_token(wn::BASE, doc, "authority");
        let authority = if self.write_owed.remove(doc) {
            wn::INGESTED.to_string()
        } else {
            held.clone().unwrap_or_else(|| wn::SEED.to_string())
        };
        let subject = doc.as_str();
        if self.net_token(wn::AUTHORITY, doc, "hash").is_none() {
            self.net
                .write(wn::AUTHORITY, subject, wn::content([("hash", wn::SEED)]));
        }
        self.net.write(
            wn::BASE,
            subject,
            wn::content([("hash", &hash), ("authority", &authority)]),
        );
        self.net
            .write(wn::ON_DISK, subject, wn::content([("hash", &hash)]));
        if held.is_none() {
            self.net.write(wn::DISK_CHANGED, subject, wn::content([]));
        }
    }

    /// [`record_base`](Self::record_base) for the document homed at
    /// `canonical`, after an ingest outside the net.
    pub(super) fn sync_base_at(&mut self, canonical: &CanonicalPath) {
        let Some(doc) = self.last_projection_doc.get(canonical).cloned() else {
            return;
        };
        if self.doc_home.get(&doc) == Some(canonical) {
            self.record_base(&doc, canonical);
        }
    }

    /// A refusal is disclosed per path, so after `doc` moved to the
    /// quarantined `home` its refused bytes are ingested again on the next
    /// read.
    pub(super) fn reread_if_refused(&mut self, doc: &EntityUri, home: &CanonicalPath) {
        if !self.quarantined.contains_key(home) {
            return;
        }
        if let Some(authority) = self.net_token(wn::BASE, doc, "authority") {
            self.net.write(
                wn::BASE,
                doc.as_str(),
                wn::content([("hash", wn::ABSENT), ("authority", &authority)]),
            );
        }
    }

    /// The net owns the file of the document homed at `canonical`.
    pub(super) fn net_owns(&self, canonical: &CanonicalPath) -> Option<EntityUri> {
        let doc = self.last_projection_doc.get(canonical)?;
        (self.doc_home.get(doc) == Some(canonical)
            && self.net_token(wn::BASE, doc, "hash").is_some())
        .then(|| doc.clone())
    }

    /// The store rendered `doc` as `rendered`: the net's `Authority` moves to
    /// it and the next `CatchUpWrite` writes it.
    pub(super) fn owe_render(
        &mut self,
        doc: &EntityUri,
        canonical: &CanonicalPath,
        rendered: String,
        images: bool,
    ) {
        if self.net_token(wn::BASE, doc, "hash").is_none() {
            self.record_base(doc, canonical);
        }
        let hash = BaseSidecars::hash(&rendered);
        let images = images || self.owed.get(doc).is_some_and(|o| o.images);
        self.owed.insert(
            doc.clone(),
            Owed {
                rendered,
                images,
                home: canonical.clone(),
            },
        );
        self.net
            .write(wn::AUTHORITY, doc.as_str(), wn::content([("hash", &hash)]));
    }

    /// Runs the net's effects until it is quiescent, then brings the empty
    /// file conditions in step with the `Refused` place.
    pub async fn drive_writeback_net(&mut self) -> Result<()> {
        let stopped = |e| anyhow::anyhow!("the write-back net stopped: {e}");
        loop {
            let requests = self.net.step().map_err(stopped)?;
            if requests.is_empty() {
                break;
            }
            for request in requests {
                let result = self.run_effect(&request).await;
                self.net.complete(&request, result).map_err(stopped)?;
            }
        }
        self.persist_undone_deletions().await?;
        self.reconcile_refused();
        self.reconcile_stalls();
        Ok(())
    }

    /// The failure of `doc`'s catch-up on its current input as an error:
    /// its file does not hold the store's text. A pending retry is a
    /// disclosed stall.
    pub(super) fn catch_up_failure(&self, doc: &EntityUri) -> Result<()> {
        for transition in wn::CATCH_UP {
            let (why_not, mark) = self.net.read(|exec| {
                (
                    exec.why_not(transition, doc.as_str()),
                    exec.mark(transition, doc.as_str()).cloned(),
                )
            });
            match why_not {
                Some(WhyNot::RetryPending { .. }) => {
                    let mark = mark.expect("a pending retry keeps its failure mark");
                    return Err(anyhow::anyhow!("{}", mark.cause.text).context(
                        super::AlreadyDisclosed(format!("write-back of {doc} not landed")),
                    ));
                }
                Some(WhyNot::FailedOnThisInput(cause)) => {
                    return Err(anyhow::anyhow!(
                        "write-back of {doc} failed: {}",
                        cause.text
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn run_effect(&mut self, request: &EffectRequest) -> EffectResult {
        let doc = doc_of(&request.subject);
        match &*request.transition {
            wn::READ_DISK => self.read_disk(&request.subject, &doc).await,
            wn::CATCH_UP_INGEST => {
                self.catch_up_ingest(&request.subject, &doc, &request.input)
                    .await
            }
            wn::CATCH_UP_WRITE => {
                self.catch_up_write(&request.subject, &doc, &request.input)
                    .await
            }
            wn::ACCEPT_REMOVAL => {
                self.accept_removal(&request.subject, &doc, &request.input)
                    .await
            }
            wn::REWRITE_FROM_STORE => self.rewrite_from_store(&request.subject, &doc).await,
            other => panic!("the write-back net has no effect for {other}"),
        }
    }

    async fn read_disk(&mut self, subject: &Subject, doc: &EntityUri) -> EffectResult {
        let home = match self.net_home(doc).await {
            Ok(home) => home,
            Err(e) => return EffectResult::Failed(Cause::definite(format!("{e:#}"))),
        };
        let Some(home) = home else {
            return EffectResult::Done(vec![on_disk(subject, wn::ABSENT)]);
        };
        match self.read_if_present(home.as_path_buf()).await {
            Ok(text) => EffectResult::Done(vec![on_disk(subject, &hash_of(text.as_deref()))]),
            Err(e) => EffectResult::Failed(Cause::retryable(format!("{e:#}"))),
        }
    }

    async fn catch_up_ingest(
        &mut self,
        subject: &Subject,
        doc: &EntityUri,
        input: &holon_net::exec::Content,
    ) -> EffectResult {
        self.owed.remove(doc);
        let home = match self.net_home(doc).await {
            Ok(Some(home)) => home,
            Ok(None) => {
                return EffectResult::Done(vec![base(
                    subject,
                    wn::ABSENT,
                    field(input, "base_authority"),
                )]);
            }
            Err(e) => return EffectResult::Failed(Cause::definite(format!("{e:#}"))),
        };
        let path = home.as_path_buf().clone();
        let outcome = self.on_file_changed_unpersisted(&path).await;
        let result = match &outcome {
            Ok(IngestOutcome::Ingested) => {
                let hash = hash_of(self.last_projection.get(&home).map(String::as_str));
                // A deleted file cascades its document away: nothing is owed.
                let authority = if self.write_owed.remove(doc) {
                    wn::INGESTED
                } else if hash == wn::ABSENT {
                    field(input, "authority")
                } else {
                    field(input, "base_authority")
                };
                EffectResult::Done(vec![
                    base(subject, &hash, authority),
                    on_disk(subject, &hash),
                ])
            }
            Ok(IngestOutcome::RefusedEmptyFile) => EffectResult::Done(vec![output(
                wn::REFUSED,
                subject,
                [
                    ("seen", field(input, "disk")),
                    ("authority", field(input, "authority")),
                ],
            )]),
            Ok(other) => EffectResult::Failed(Cause::definite(format!(
                "{} was not ingested: {other:?}",
                path.display()
            ))),
            // A read that failed partway may land on the next try.
            Err(e) if self.quarantined.get(&home) == Some(&super::QuarantineCause::Ingest) => {
                EffectResult::Failed(Cause::retryable(format!("{e:#}")))
            }
            Err(e) if self.quarantined.contains_key(&home) => {
                info!(
                    "[FileSyncController] {} refused on read ({e:#}); its write-back waits for \
                     new file content",
                    path.display()
                );
                EffectResult::Done(vec![base(subject, field(input, "disk"), wn::REFUSED_READ)])
            }
            Err(e) => EffectResult::Failed(Cause::definite(format!("{e:#}"))),
        };
        self.ingest_results.insert(doc.clone(), outcome);
        result
    }

    async fn catch_up_write(
        &mut self,
        subject: &Subject,
        doc: &EntityUri,
        input: &holon_net::exec::Content,
    ) -> EffectResult {
        let disk = field(input, "disk");
        let base_hash = field(input, "base");
        let authority = field(input, "authority");
        let unchanged = || EffectResult::Done(vec![base(subject, base_hash, authority)]);
        if !self.home_is_writable(doc) {
            self.owed.remove(doc);
            return unchanged();
        }
        // An owed render goes to the path it was rendered for; an ingest's
        // normalization goes back to the file it read.
        let normalizes = field(input, "base_authority") == wn::INGESTED;
        let known_home = match self.owed.get(doc) {
            Some(owed) => Some(owed.home.clone()),
            None => self.doc_home.get(doc).filter(|_| normalizes).cloned(),
        };
        let known_home = known_home.map(|h| {
            super::VaultPath::inside(&self.root_dir, h.as_path_buf().clone())
                .expect("a document's home lies inside the vault")
        });
        let derived = match known_home {
            Some(home) => Ok(Some(home)),
            None => self.doc_id_to_path(doc, PathIntent::WriteOwnFile).await,
        };
        let vault_path = match derived {
            Ok(Some(p)) => p,
            Ok(None) => return unchanged(),
            Err(e) => {
                self.disclose_derivation_failure(
                    doc,
                    &e,
                    "write-back: this document's edits are NOT written to disk",
                );
                return EffectResult::Failed(Cause::definite(format!("{e:#}")));
            }
        };
        let path = vault_path.as_path().to_path_buf();
        let canonical = CanonicalPath::new(&path);
        let at_home = self.doc_home.get(doc).is_none_or(|h| *h == canonical);
        let basis = match read_basis(&self.fs, &path).await {
            Ok(b) => b,
            Err(e) => return EffectResult::Failed(Cause::retryable(format!("{e:#}"))),
        };
        if at_home && hash_of(basis.content.as_deref()) != disk {
            return EffectResult::Done(vec![disk_changed(subject)]);
        }
        let disk_text = basis.content.clone().unwrap_or_default();
        let rendered = match self.owed.get(doc) {
            Some(owed) => owed.rendered.clone(),
            None => {
                let rendered = match self.render_file_by_doc_id(doc, &path).await {
                    Ok(r) => r,
                    Err(e) => return EffectResult::Failed(Cause::definite(format!("{e:#}"))),
                };
                match self.keep_copies(&path, &canonical, rendered, &disk_text) {
                    Ok(r) => r,
                    Err(e) => return EffectResult::Failed(Cause::definite(format!("{e:#}"))),
                }
            }
        };
        if at_home && BaseSidecars::hash(&rendered) == base_hash {
            self.owed.remove(doc);
            return unchanged();
        }
        if self.gate_virtual_seed_write(doc, &canonical, &rendered, !disk_text.is_empty()) {
            self.owed.remove(doc);
            return unchanged();
        }
        let sanctioned = self.pending_removals.get(doc).cloned().unwrap_or_default();
        match self.quarantined.get(&canonical).copied() {
            Some(QuarantineCause::Ingest | QuarantineCause::Refused) => {
                self.note_quarantine_skip(&path);
                return unchanged();
            }
            Some(QuarantineCause::WritebackVeto) => {
                match self
                    .writeback_render_is_grounded(&path, &disk_text, &rendered, &sanctioned)
                    .await
                {
                    Ok(true) => self.clear_writeback_quarantine(&canonical, &path),
                    Ok(false) => {
                        self.note_quarantine_skip(&path);
                        return unchanged();
                    }
                    Err(e) => return EffectResult::Failed(Cause::definite(format!("{e:#}"))),
                }
            }
            None => {
                if let Err(e) = self
                    .veto_ungrounded_removals(&path, &disk_text, &rendered, &sanctioned)
                    .await
                {
                    return EffectResult::Failed(Cause::definite(format!("{e:#}")));
                }
            }
        }
        let written = self
            .write_back_or_skip_readonly(doc, &vault_path, &basis, &rendered, !normalizes)
            .await;
        match written {
            Err(e) => {
                let cause = format!("{e:#}");
                self.disclose_stall(doc, &path, &cause);
                EffectResult::Failed(Cause::retryable(cause))
            }
            Ok(ProjectionWrite::SkippedReadOnly)
                if self.writeback_readonly.contains(&canonical) =>
            {
                let cause = format!("{} is on a read-only file system", path.display());
                self.disclose_stall(doc, &path, &cause);
                EffectResult::Failed(Cause::retryable(cause))
            }
            Ok(ProjectionWrite::SkippedReadOnly) => {
                self.owed.remove(doc);
                unchanged()
            }
            Ok(ProjectionWrite::Changed) => match self.read_if_present(&path).await {
                Ok(now) if now.as_deref() == basis.content.as_deref() => {
                    let cause = format!(
                        "Holon did not write {}: its stamp keeps changing while its bytes do \
                         not; some process keeps rewriting it",
                        path.display()
                    );
                    self.disclose_stall(doc, &path, &cause);
                    EffectResult::Failed(Cause::retryable(cause))
                }
                Ok(_) => EffectResult::Done(vec![disk_changed(subject)]),
                Err(e) => EffectResult::Failed(Cause::retryable(format!("{e:#}"))),
            },
            Ok(ProjectionWrite::Written) => {
                self.run_post_write_hook(&path);
                let owed = self.owed.remove(doc);
                if owed.is_some_and(|o| o.images) {
                    if let Err(e) = self.materialize_images(doc).await {
                        tracing::error!(
                            "[FileSyncController] {} was written, but its images were not \
                             materialized: {e:#}",
                            path.display()
                        );
                    }
                }
                #[cfg(feature = "crash-injection")]
                if field(input, "base_authority") == wn::INGESTED {
                    crate::crash_injection::reached("after_ingest_write_back");
                }
                self.pending_removals.remove(doc);
                if let Some((old, _)) = self.stall_disclosed.remove(doc) {
                    if let Some(disclosure) = &self.writeback_disclosure {
                        disclosure.writeback_resumed(&old);
                    }
                }
                // An ingest's normalization write records its hash at once,
                // as the ingest itself does, so the next boot skips the file.
                if field(input, "base_authority") == wn::INGESTED {
                    let rel_path = path
                        .strip_prefix(&self.root_dir)
                        .expect("a written vault path lies under the vault root")
                        .to_path_buf();
                    let members = self
                        .persisted_read_only_blocks
                        .get(&canonical)
                        .cloned()
                        .unwrap_or_default();
                    let projection_hash = self.projection_hash(&rendered);
                    if let Err(e) = self
                        .persist_disk_hash_for(
                            &canonical,
                            &rel_path,
                            &projection_hash,
                            &rendered,
                            doc,
                            &members,
                        )
                        .await
                    {
                        return EffectResult::Failed(Cause::retryable(format!("{e:#}")));
                    }
                }
                let hash = BaseSidecars::hash(&rendered);
                self.last_projection.insert(canonical, rendered);
                info!("[FileSyncController] Wrote {}", path.display());
                EffectResult::Done(vec![
                    base(subject, &hash, authority),
                    on_disk(subject, &hash),
                ])
            }
        }
    }

    fn disclose_stall(&mut self, doc: &EntityUri, path: &Path, cause: &str) {
        warn!(path = %path.display(), "[FileSyncController] write-back not landed: {cause}");
        let Some(disclosure) = &self.writeback_disclosure else {
            return;
        };
        if let Some((old, _)) = self.stall_disclosed.get(doc).filter(|(old, _)| old != path) {
            disclosure.writeback_resumed(old);
        }
        disclosure.writeback_stalled(path, cause);
        self.stall_disclosed
            .insert(doc.clone(), (path.to_path_buf(), cause.to_string()));
    }

    /// A stall whose catch-up no longer carries a retryable mark landed or
    /// was superseded, so its condition is lifted.
    fn reconcile_stalls(&mut self) {
        let failing: Vec<String> = self.net.read(|exec| {
            exec.marks()
                .filter(|(t, _, mark)| wn::CATCH_UP.contains(t) && mark.cause.retryable)
                .map(|(_, doc, _)| doc.to_string())
                .collect()
        });
        let over: Vec<EntityUri> = self
            .stall_disclosed
            .keys()
            .filter(|doc| !failing.iter().any(|f| f == doc.as_str()))
            .cloned()
            .collect();
        for doc in over {
            let (path, _) = self.stall_disclosed.remove(&doc).expect("listed above");
            if let Some(disclosure) = &self.writeback_disclosure {
                disclosure.writeback_resumed(&path);
            }
        }
    }

    /// The `vault-file-emptied` condition shows exactly the documents the
    /// `Refused` place holds.
    fn reconcile_refused(&mut self) {
        let refused: Vec<(String, String)> = self.net.read(|exec| {
            wn::refused_docs(exec)
                .into_iter()
                .map(|doc| {
                    let seen = field(
                        &exec
                            .tokens()
                            .token(wn::REFUSED, &doc)
                            .expect("listed by refused_docs")
                            .content,
                        "seen",
                    )
                    .to_string();
                    (doc, seen)
                })
                .collect()
        });
        let lifted: Vec<EntityUri> = self
            .refused_disclosed
            .keys()
            .filter(|doc| !refused.iter().any(|(r, _)| r == doc.as_str()))
            .cloned()
            .collect();
        for doc in lifted {
            let (path, _) = self.refused_disclosed.remove(&doc).expect("listed above");
            if let Some(disclosure) = &self.writeback_disclosure {
                disclosure.file_condition_cleared(&path, ConditionKind::VAULT_FILE_EMPTIED);
            }
        }
        for (raw, seen) in refused {
            let doc = doc_of(&Subject::new(&raw));
            let Some(home) = self.doc_home.get(&doc).map(|h| h.as_path_buf().clone()) else {
                continue;
            };
            let entry = (home.clone(), seen.clone());
            if self.refused_disclosed.get(&doc) == Some(&entry) {
                continue;
            }
            if let Some(disclosure) = &self.writeback_disclosure {
                disclosure.vault_file_emptied(&home, &seen);
            }
            self.refused_disclosed.insert(doc, entry);
        }
    }

    async fn accept_removal(
        &mut self,
        subject: &Subject,
        doc: &EntityUri,
        input: &holon_net::exec::Content,
    ) -> EffectResult {
        if field(input, "authority") != field(input, "at_refusal") {
            let home = self.doc_home.get(doc).map(|h| h.as_path_buf().clone());
            let result = self.rewrite_from_store(subject, doc).await;
            if let (Some(home), Some(disclosure)) = (home, &self.writeback_disclosure) {
                disclosure.removal_not_accepted(
                    &home,
                    "the document changed in Holon after the file was emptied, so Holon \
                     rewrote the file from the store instead",
                );
            }
            return result;
        }
        let Some(home) = self.doc_home.get(doc).cloned() else {
            return EffectResult::Done(vec![disk_changed(subject)]);
        };
        let path = home.as_path_buf().clone();
        self.accepting_removal = Some(home.clone());
        let outcome = self.on_file_changed_unpersisted(&path).await;
        self.accepting_removal = None;
        let persisted = self.persist_undone_deletions().await;
        match (outcome, persisted) {
            (Ok(IngestOutcome::Ingested), Ok(())) => {
                if let Some(disclosure) = &self.writeback_disclosure {
                    disclosure.file_condition_cleared(&path, ConditionKind::REMOVAL_NOT_ACCEPTED);
                }
                let hash = hash_of(self.last_projection.get(&home).map(String::as_str));
                EffectResult::Done(vec![
                    base(subject, &hash, wn::INGESTED),
                    on_disk(subject, &hash),
                ])
            }
            (outcome, persisted) => {
                let cause = format!("{outcome:?} / {persisted:?}");
                if let Some(disclosure) = &self.writeback_disclosure {
                    disclosure.removal_not_accepted(&path, &cause);
                }
                EffectResult::Done(vec![disk_changed(subject)])
            }
        }
    }

    async fn rewrite_from_store(&mut self, subject: &Subject, doc: &EntityUri) -> EffectResult {
        let landed = self.rewrite_doc(doc).await;
        let home = self.doc_home.get(doc).map(|h| h.as_path_buf().clone());
        match landed {
            Ok(hash) => {
                if let (Some(home), Some(disclosure)) = (&home, &self.writeback_disclosure) {
                    disclosure
                        .file_condition_cleared(home, ConditionKind::REWRITE_FROM_STORE_FAILED);
                }
                let authority = self
                    .net_token(wn::AUTHORITY, doc, "hash")
                    .expect("a refused document has an Authority");
                EffectResult::Done(vec![
                    base(subject, &hash, &authority),
                    on_disk(subject, &hash),
                ])
            }
            Err(e) => {
                if let (Some(home), Some(disclosure)) = (&home, &self.writeback_disclosure) {
                    disclosure.rewrite_from_store_failed(home, &format!("{e:#}"));
                }
                EffectResult::Done(vec![disk_changed(subject)])
            }
        }
    }

    /// Writes the store's render of `doc` over its file; the hash written.
    async fn rewrite_doc(&mut self, doc: &EntityUri) -> Result<String> {
        let vault_path = self
            .doc_id_to_path(doc, PathIntent::WriteOwnFile)
            .await?
            .ok_or_else(|| anyhow::anyhow!("{doc} names no file to rewrite"))?;
        let path: PathBuf = vault_path.as_path().to_path_buf();
        let canonical = CanonicalPath::new(&path);
        let basis = read_basis(&self.fs, &path).await?;
        let rendered = self.render_file_by_doc_id(doc, &path).await?;
        let rendered = self.keep_copies(
            &path,
            &canonical,
            rendered,
            basis.content.as_deref().unwrap_or(""),
        )?;
        match self
            .write_back_or_skip_readonly(doc, &vault_path, &basis, &rendered, true)
            .await?
        {
            ProjectionWrite::Written => {
                self.run_post_write_hook(&path);
                self.owed.remove(doc);
                let hash = BaseSidecars::hash(&rendered);
                self.last_projection.insert(canonical, rendered);
                Ok(hash)
            }
            other => anyhow::bail!("{} was not rewritten: {other:?}", path.display()),
        }
    }
}
