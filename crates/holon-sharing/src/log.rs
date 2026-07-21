//! The owner-scoped, totally-ordered crossing log (ADR 0028 H2).
//!
//! Both **boundary crossings and policy edits** enter the ONE log (review A1 —
//! one arbitration rule, one code path). The log is backed by its own dedicated
//! `LoroDoc` with its own Lamport clock (Inc 0(b) clock-domain decision:
//! candidate (i), the log is its own doc/sequence — one clock, not
//! per-container).
//!
//! Each entry is stamped at append time with `(lamport_height(log_doc) read
//! immediately before the push, stable_peer_id)` — the FROZEN ordering tuple —
//! and a structurally-present [`OwnerSig`]. Because the stamp travels *inside*
//! the merged entry, the total order is a pure function of merged log content:
//! no post-merge oplog re-read, no consensus round-trip.

use loro::ExportMode;
use loro::LoroDoc;

use crate::types::Crossing;
use crate::types::CrossingKey;
use crate::types::LogEntry;
use crate::types::LogEntryBody;
use crate::types::OwnerSig;
use crate::types::SigningAuthority;
use crate::types::StablePeerId;

const LOG_LIST: &str = "crossing_log";

/// A malformed `supersedes` witness rejected at the SOURCE (fail-loud, never
/// swallowed). Mirrors the read-boundary defect classes in
/// [`crate::arbitration`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WitnessError {
    #[error(
        "crossing `{id}` supersedes unknown crossing `{target}` — the witness \
         must reference a crossing already in this device's log"
    )]
    UnknownTarget { id: String, target: String },
    #[error(
        "crossing `{id}` on block `{block}` supersedes `{target}` which is on a \
         DIFFERENT block `{target_block}` — a witness may only supersede a \
         crossing of the SAME block"
    )]
    WrongBlock {
        id: String,
        block: String,
        target: String,
        target_block: String,
    },
}

/// The crossing log for one device's view of one owner scope.
pub struct CrossingLog {
    doc: LoroDoc,
    peer: StablePeerId,
    authority: Box<dyn SigningAuthority>,
}

/// Canonical bytes an entry is signed over: `(key, body)` serialized
/// deterministically. The signature does NOT cover itself.
fn canonical_bytes(key: &CrossingKey, body: &LogEntryBody) -> Vec<u8> {
    serde_json::to_vec(&(key, body)).expect("log entry body is serializable")
}

impl CrossingLog {
    /// Open a fresh log for `peer`. The backing `LoroDoc`'s peer id is set to
    /// the stable peer id so two devices author under distinct Loro actors and
    /// their concurrent appends never collide counters.
    pub fn new(peer: StablePeerId, authority: Box<dyn SigningAuthority>) -> Self {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer.0)
            .expect("fresh doc accepts a peer id before any op");
        Self {
            doc,
            peer,
            authority,
        }
    }

    fn append(&self, body: LogEntryBody) -> CrossingKey {
        // Lamport read IMMEDIATELY before the push = the lamport the new op will
        // receive (Inc 0(b)). Concurrent appends from a shared base read the
        // same height => equal lamport => the peer tiebreak decides.
        let lamport = holon_loro::loro_backend::doc_lamport_height(&self.doc);
        let key = CrossingKey {
            lamport,
            peer: self.peer,
        };
        let sig: OwnerSig = self.authority.sign(&canonical_bytes(&key, &body));
        let entry = LogEntry { key, body, sig };
        let json = serde_json::to_string(&entry).expect("entry is serializable");
        let list = self.doc.get_list(LOG_LIST);
        list.push(json.as_str())
            .expect("appending a string to a Loro list cannot fail");
        self.doc.commit();
        key
    }

    /// Append a bracket entry. Public so the migration driver can bracket a
    /// crossing (Begin before delete-in-source, Commit after create-in-target)
    /// and record policy edits.
    ///
    /// This is the RAW path: it performs NO `supersedes` validation. It models
    /// entries that arrive over merge from another device (which may be buggy
    /// or lying). Malformed witnesses are caught defensively at the read
    /// boundary by [`crate::arbitration::arbitrate`]. Prefer
    /// [`append_crossing`] when THIS device authors a crossing.
    pub fn append_entry(&self, body: LogEntryBody) -> CrossingKey {
        self.append(body)
    }

    /// Source-side authoring of a crossing (SOURCE guard, verifier fix):
    /// rejects a malformed `supersedes` witness LOUDLY before it ever
    /// enters the log, so a migration-driver bug minting a wrong
    /// `CrossingId` fails at its origin rather than silently corrupting
    /// arbitration. A witness must reference a crossing of the SAME block
    /// that already exists in this device's log (the author observed it).
    /// Returns the crossing's [`CrossingKey`] on success.
    pub fn append_crossing(&self, crossing: Crossing) -> Result<CrossingKey, WitnessError> {
        if let Some(sid) = &crossing.supersedes {
            let entries = self.entries();
            let target = entries.iter().find_map(|e| match &e.body {
                LogEntryBody::CrossingBegin(c) if &c.crossing_id == sid => Some(c),
                _ => None,
            });
            match target {
                None => {
                    return Err(WitnessError::UnknownTarget {
                        id: crossing.crossing_id.0.clone(),
                        target: sid.0.clone(),
                    });
                }
                Some(t) if t.block != crossing.block => {
                    return Err(WitnessError::WrongBlock {
                        id: crossing.crossing_id.0.clone(),
                        block: crossing.block.0.clone(),
                        target: sid.0.clone(),
                        target_block: t.block.0.clone(),
                    });
                }
                Some(_) => {}
            }
        }
        Ok(self.append(LogEntryBody::CrossingBegin(crossing)))
    }

    /// All entries this device currently holds, sorted by the frozen tuple
    /// (ascending; max wins). Deserialized from the backing doc every call —
    /// the doc is the source of truth, this method never caches stale state.
    pub fn entries(&self) -> Vec<LogEntry> {
        let list = self.doc.get_list(LOG_LIST);
        let mut out = Vec::with_capacity(list.len());
        for i in 0..list.len() {
            let value = list.get(i).expect("index < len is in bounds");
            let loro_value = value
                .into_value()
                .expect("log element is a plain value, not a container");
            let s = loro_value.into_string().expect("log element is a string");
            let entry: LogEntry =
                serde_json::from_str(s.as_str()).expect("stored entry round-trips");
            out.push(entry);
        }
        out.sort_by_key(|e| e.key);
        out
    }

    /// Bidirectional delta exchange with another replica of the same owner log
    /// (the deterministic equivalent of two devices reaching merged state).
    /// After this both logs hold the union of entries and, because every stamp
    /// travels in-payload, [`entries`](Self::entries) yields the identical
    /// total order on both.
    pub fn merge(&self, other: &CrossingLog) {
        merge_docs(&self.doc, &other.doc);
    }

    /// This device's stable peer id (the ordering-tuple component it stamps).
    pub fn peer(&self) -> StablePeerId {
        self.peer
    }
}

/// Symmetric CRDT delta exchange — mirrors `loro_backend`'s `sync_pair` without
/// pulling in the `test-helpers`-gated `multi_peer::sync_docs_direct`.
fn merge_docs(a: &LoroDoc, b: &LoroDoc) {
    let a_delta = a
        .export(ExportMode::updates(&b.oplog_vv()))
        .expect("export delta");
    if !a_delta.is_empty() {
        b.import(&a_delta).expect("import delta");
    }
    let b_delta = b
        .export(ExportMode::updates(&a.oplog_vv()))
        .expect("export delta");
    if !b_delta.is_empty() {
        a.import(&b_delta).expect("import delta");
    }
}
