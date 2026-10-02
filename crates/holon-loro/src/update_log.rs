//! Append-only log of Loro update blobs kept beside a document's snapshot
//! (`<snapshot>.log`). The persisted document is the snapshot plus every
//! record in its log.
//!
//! Layout: [`MAGIC`], then records of
//! `[u32 LE length][u32 LE crc32 of the length bytes][u32 LE crc32 of the
//! payload][payload]`. The checksums tell damage from a crash: a crash can only
//! cut the file short, so a record that is whole inside the file yet fails its
//! checksum was damaged, and replay refuses it instead of dropping what
//! follows.

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use loro::LoroDoc;

pub const MAGIC: &[u8; 8] = b"HOLONUL2";
/// Bytes a record adds to the log beyond its blob.
pub const RECORD_HEADER: u64 = 12;

pub fn log_path(snapshot: &Path) -> PathBuf {
    let mut name = snapshot
        .file_name()
        .expect("a snapshot path names a file")
        .to_os_string();
    name.push(".log");
    snapshot.with_file_name(name)
}

/// A stump the log held behind its last acknowledged record, cut off before
/// an append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StumpCut {
    pub bytes: u64,
}

/// Append one record and make it durable before returning. The log must
/// exist: only [`reset`] creates one. `acked_len` is the file length after
/// the last acknowledged write; bytes beyond it are a stump and are cut off
/// first. A failed append cuts its own partial bytes off again, so the log
/// always ends at the last acknowledged record.
#[cfg(not(target_arch = "wasm32"))]
pub fn append(log: &Path, blob: &[u8], acked_len: u64) -> Result<Option<StumpCut>> {
    append_with(log, blob, acked_len, |file, record| file.write_all(record))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn append_with(
    log: &Path,
    blob: &[u8],
    acked_len: u64,
    write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> Result<Option<StumpCut>> {
    let len = u32::try_from(blob.len())
        .with_context(|| format!("a {}-byte update does not fit a log record", blob.len()))?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(log)
        .with_context(|| format!("opening the Loro update log {}", log.display()))?;
    let on_disk = file
        .metadata()
        .with_context(|| format!("reading the length of {}", log.display()))?
        .len();
    anyhow::ensure!(
        on_disk >= acked_len,
        "the Loro update log {} is {on_disk} bytes but {acked_len} bytes were acknowledged",
        log.display()
    );
    let stump = (on_disk > acked_len).then_some(StumpCut {
        bytes: on_disk - acked_len,
    });
    if stump.is_some() {
        cut_to(&file, log, acked_len)?;
    }
    let mut record = Vec::with_capacity(blob.len() + RECORD_HEADER as usize);
    let len_bytes = len.to_le_bytes();
    record.extend_from_slice(&len_bytes);
    record.extend_from_slice(&crc32fast::hash(&len_bytes).to_le_bytes());
    record.extend_from_slice(&crc32fast::hash(blob).to_le_bytes());
    record.extend_from_slice(blob);
    if let Err(e) = write(&mut file, &record)
        .and_then(|()| holon_filesystem::fs_port::sync_data_blocking(&file, log))
    {
        let appended = anyhow::Error::new(e).context(format!(
            "appending to the Loro update log {}",
            log.display()
        ));
        return Err(match cut_to(&file, log, acked_len) {
            Ok(()) => appended,
            Err(cut) => appended.context(format!("and the partial record stays: {cut:#}")),
        });
    }
    Ok(stump)
}

#[cfg(not(target_arch = "wasm32"))]
fn cut_to(file: &std::fs::File, log: &Path, len: u64) -> Result<()> {
    file.set_len(len)
        .and_then(|()| holon_filesystem::fs_port::sync_data_blocking(file, log))
        .with_context(|| format!("cutting {} back to {len} bytes", log.display()))
}

/// Durably replace the log with an empty one.
#[cfg(not(target_arch = "wasm32"))]
pub fn reset(log: &Path) -> Result<()> {
    holon_filesystem::fs_port::write_durable_blocking(log, MAGIC)
        .with_context(|| format!("resetting the Loro update log {}", log.display()))
}

/// Whether the log at `log` holds any record. A missing log holds none.
pub fn holds_records(log: &Path) -> Result<bool> {
    match std::fs::metadata(log) {
        Ok(meta) => Ok(meta.len() > MAGIC.len() as u64),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("reading the Loro update log {}", log.display())),
    }
}

/// A last record that a crash cut short or left zero-filled; it was never
/// acknowledged, so dropping it loses nothing a caller was told is saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TornTail {
    pub offset: u64,
    pub bytes: u64,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Replay {
    pub records: usize,
    pub torn_tail: Option<TornTail>,
    /// The log file holds no complete magic: it was created and never synced.
    pub magic_incomplete: bool,
}

/// Import every record of the log at `log` into `doc`, which holds the
/// snapshot the log belongs to. A missing log is an empty one.
pub fn replay(doc: &LoroDoc, log: &Path) -> Result<Replay> {
    let bytes = match std::fs::read(log) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Replay::default()),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("reading the Loro update log {}", log.display()));
        }
    };
    let rebuild = || {
        format!(
            "delete the Loro store {} to rebuild it from the org files",
            log.parent().unwrap_or(log).display()
        )
    };
    if bytes.len() < MAGIC.len() && MAGIC.starts_with(&bytes) {
        return Ok(Replay {
            magic_incomplete: true,
            ..Replay::default()
        });
    }
    if !bytes.starts_with(MAGIC) {
        bail!(
            "{} is not a Loro update log (its first bytes are not {MAGIC:?}); {}",
            log.display(),
            rebuild()
        );
    }
    let mut replay = Replay::default();
    let mut at = MAGIC.len();
    while at < bytes.len() {
        let rest = &bytes[at..];
        let torn = |reason: String| TornTail {
            offset: at as u64,
            bytes: rest.len() as u64,
            reason,
        };
        // A size-extending append whose data never landed reads back as zeros.
        if rest.iter().all(|b| *b == 0) {
            replay.torn_tail = Some(torn(format!("{} zero bytes", rest.len())));
            break;
        }
        let Some(header) = rest.get(..RECORD_HEADER as usize) else {
            replay.torn_tail = Some(torn(format!("{} bytes of a record header", rest.len())));
            break;
        };
        let word = |i: usize| u32::from_le_bytes(header[i..i + 4].try_into().expect("four bytes"));
        if crc32fast::hash(&header[..4]) != word(4) {
            bail!(
                "the record header at byte {at} of {} fails its checksum, so the log is damaged, \
                 not cut short; {}",
                log.display(),
                rebuild()
            );
        }
        let len = word(0) as usize;
        let end = RECORD_HEADER as usize + len;
        let Some(blob) = rest.get(RECORD_HEADER as usize..end) else {
            replay.torn_tail = Some(torn(format!(
                "{} of the record's {len} bytes",
                rest.len() - RECORD_HEADER as usize
            )));
            break;
        };
        if crc32fast::hash(blob) != word(8) {
            bail!(
                "the record at byte {at} of {} fails its checksum although all {len} of its \
                 bytes are present, so the log is damaged, not cut short; {}",
                log.display(),
                rebuild()
            );
        }
        LoroDoc::decode_import_blob_meta(blob, true).with_context(|| {
            format!(
                "the record at byte {at} of {} passes its checksum but is not a Loro update; {}",
                log.display(),
                rebuild()
            )
        })?;
        // A record the snapshot already holds (a crash between writing a
        // snapshot and resetting its log) imports as a no-op.
        let status = doc.import(blob).with_context(|| {
            format!(
                "importing the record at byte {at} of {}; {}",
                log.display(),
                rebuild()
            )
        })?;
        if let Some(pending) = status.pending {
            bail!(
                "the record at byte {at} of {} depends on changes neither the snapshot nor the \
                 log holds ({pending:?}); {}",
                log.display(),
                rebuild()
            );
        }
        replay.records += 1;
        at += end;
    }
    Ok(replay)
}

/// The document a snapshot and its log persist, read without a store.
pub fn read_persisted(snapshot: &Path) -> Result<(LoroDoc, Replay)> {
    let bytes =
        std::fs::read(snapshot).with_context(|| format!("reading {}", snapshot.display()))?;
    let doc = LoroDoc::new();
    doc.import(&bytes)
        .with_context(|| format!("importing the snapshot {}", snapshot.display()))?;
    let replay = replay(&doc, &log_path(snapshot))?;
    Ok((doc, replay))
}
