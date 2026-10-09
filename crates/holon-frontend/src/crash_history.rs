//! Every crash record Holon keeps, read from the record dir alone, so it can
//! be shown while the engine is down. Reading moves, counts and deletes
//! nothing: only [`panic_record::seen_on`] moves a record.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use holon_api::DroppedPanics;
use holon_api::Value;
use holon_api::render_types::RenderExpr;

use crate::panic_record;
use crate::panic_record::PanicRecord;

/// Where a record sits, which says whether a start has shown it yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// [`panic_record::RECORD_FILE`]: a panic of the running process.
    ThisRun,
    /// [`panic_record::UNSHOWN_DIR`].
    Unshown,
    /// [`panic_record::SEEN_DIR`].
    Seen,
}

impl Kept {
    pub fn label(self) -> &'static str {
        match self {
            Kept::ThisRun => "this run",
            Kept::Unshown => "not yet shown",
            Kept::Seen => "shown",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Kept::ThisRun => "this-run",
            Kept::Unshown => "unshown",
            Kept::Seen => "seen",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CrashEntry {
    pub kept: Kept,
    /// The record's number within its dir; numbers repeat across dirs.
    pub n: u64,
    /// When the run ended: the file's mtime, or why it cannot be read.
    pub ended: Result<DateTime<Utc>, String>,
    pub record: PanicRecord,
}

/// The count of records dropped from one dir to keep it bounded.
#[derive(Clone, Debug)]
pub struct DroppedEntry {
    pub kept: Kept,
    pub summary: DroppedPanics,
}

/// A file or dir among the records that cannot be read as one.
#[derive(Clone, Debug)]
pub struct Unreadable {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct CrashHistory {
    /// `None` when no record dir was armed in this process.
    pub record_dir: Option<PathBuf>,
    /// Newest first: this run's, then the unshown ones, then the shown ones.
    pub records: Vec<CrashEntry>,
    pub dropped: Vec<DroppedEntry>,
    pub unreadable: Vec<Unreadable>,
}

/// One entry as the view and the text export show it.
struct Line {
    id: String,
    headline: String,
    body: String,
}

const NO_RECORD_DIR: &str = "no record dir was armed in this process, so no crash is recorded";

impl CrashHistory {
    /// The history in the dir this process records its panics in.
    pub fn of_this_process() -> Self {
        match panic_record::record_dir() {
            Some(dir) => Self::read(&dir),
            None => Self {
                record_dir: None,
                records: Vec::new(),
                dropped: Vec::new(),
                unreadable: Vec::new(),
            },
        }
    }

    /// Every entry that cannot be read becomes an [`Unreadable`] one.
    pub fn read(record_dir: &Path) -> Self {
        let mut history = Self {
            record_dir: Some(record_dir.to_path_buf()),
            records: Vec::new(),
            dropped: Vec::new(),
            unreadable: Vec::new(),
        };
        let this_run = record_dir.join(panic_record::RECORD_FILE);
        if std::fs::symlink_metadata(&this_run).is_ok() {
            match panic_record::read_record(&this_run) {
                Ok(record) => history
                    .records
                    .push(entry(Kept::ThisRun, 0, &this_run, record)),
                Err(reason) => history.unreadable.push(Unreadable {
                    path: this_run,
                    reason,
                }),
            }
        }
        // Unshown before seen: a record `seen_on` moves meanwhile is listed
        // twice, never missed, and the copy in the seen dir is dropped.
        history.read_dir(record_dir, panic_record::UNSHOWN_DIR, Kept::Unshown);
        history.read_dir(record_dir, panic_record::SEEN_DIR, Kept::Seen);
        history.read_summary(record_dir, panic_record::DROPPED_FILE, Kept::Unshown);
        history.read_summary(record_dir, panic_record::SEEN_DROPPED_FILE, Kept::Seen);
        history
    }

    fn read_dir(&mut self, record_dir: &Path, name: &str, kept: Kept) {
        let dir = record_dir.join(name);
        let listed = match panic_record::list_records(&dir) {
            Ok(listed) => listed,
            Err(e) => {
                self.unreadable.push(Unreadable {
                    path: dir,
                    reason: format!("it cannot be listed: {e}"),
                });
                return;
            }
        };
        for (n, path, record) in listed.records.into_iter().rev() {
            let entry = entry(kept, n, &path, record);
            let moved_meanwhile = kept == Kept::Seen
                && self.records.iter().any(|e| {
                    e.kept == Kept::Unshown && e.record == entry.record && e.ended == entry.ended
                });
            if !moved_meanwhile {
                self.records.push(entry);
            }
        }
        self.unreadable.extend(
            listed
                .strays
                .into_iter()
                .map(|(path, reason)| Unreadable { path, reason }),
        );
    }

    fn read_summary(&mut self, record_dir: &Path, file: &str, kept: Kept) {
        match panic_record::summary_slot(record_dir, file) {
            Ok(slot) => {
                self.unreadable
                    .extend(slot.unreadable.into_iter().map(|(path, e)| Unreadable {
                        path,
                        reason: e.to_string(),
                    }));
                if let Some(summary) = slot.summary {
                    self.dropped.push(DroppedEntry { kept, summary });
                }
            }
            Err(e) => self.unreadable.push(Unreadable {
                path: record_dir.join(file),
                reason: e.to_string(),
            }),
        }
    }

    fn lines(&self) -> Vec<Line> {
        let Some(dir) = &self.record_dir else {
            return vec![Line {
                id: "crash-record:no-dir".to_string(),
                headline: "Crash records unavailable".to_string(),
                body: NO_RECORD_DIR.to_string(),
            }];
        };
        let mut lines: Vec<Line> = self
            .unreadable
            .iter()
            .enumerate()
            .map(|(i, u)| Line {
                id: format!("crash-record:unreadable-{i}"),
                headline: format!("Unreadable: {}", u.path.display()),
                body: u.reason.clone(),
            })
            .collect();
        lines.extend(self.records.iter().map(|e| Line {
            id: format!("crash-record:{}-{}", e.kept.id(), e.n),
            headline: format!(
                "{} \u{b7} {} \u{b7} {} \u{b7} at {}",
                e.kept.label(),
                match &e.ended {
                    Ok(t) => format!("ended {}", t.format("%Y-%m-%d %H:%M:%S UTC")),
                    Err(reason) => format!("ended at an unknown time ({reason})"),
                },
                e.record.thread,
                e.record.location
            ),
            body: e.record.message.clone(),
        }));
        lines.extend(self.dropped.iter().map(|d| Line {
            id: format!("crash-record:dropped-{}", d.kept.id()),
            headline: format!("Dropped ({})", d.kept.label()),
            body: dropped_text(d),
        }));
        if lines.is_empty() {
            lines.push(Line {
                id: "crash-record:none".to_string(),
                headline: "No crash records".to_string(),
                body: format!("Holon has recorded no crash in {}.", dir.display()),
            });
        }
        lines
    }

    /// The headline of every entry that is not a record.
    pub fn other_headlines(&self) -> Vec<String> {
        let records: Vec<String> = self
            .records
            .iter()
            .map(|e| format!("crash-record:{}-{}", e.kept.id(), e.n))
            .collect();
        self.lines()
            .into_iter()
            .filter(|line| line.id != "crash-record:none" && !records.contains(&line.id))
            .map(|line| line.headline)
            .collect()
    }

    /// The data rows [`render_expr`] lays out, one per entry.
    pub fn rows(&self) -> Vec<Arc<HashMap<String, Value>>> {
        self.lines()
            .into_iter()
            .map(|line| {
                Arc::new(HashMap::from([
                    ("id".to_string(), Value::String(line.id)),
                    ("headline".to_string(), Value::String(line.headline)),
                    ("body".to_string(), Value::String(line.body)),
                ]))
            })
            .collect()
    }

    /// The whole history as plain text, for the clipboard and for agents.
    pub fn to_text(&self) -> String {
        let dir = self
            .record_dir
            .as_ref()
            .map_or_else(|| "no record dir".to_string(), |d| d.display().to_string());
        let mut text = format!("Holon crash history ({dir}), newest first\n");
        for line in self.lines() {
            text.push_str(&format!("\n{}\n{}\n", line.headline, line.body));
        }
        text
    }
}

fn entry(kept: Kept, n: u64, path: &Path, record: PanicRecord) -> CrashEntry {
    CrashEntry {
        kept,
        n,
        ended: panic_record::ended_at(path).map_err(|e| e.to_string()),
        record,
    }
}

fn dropped_text(dropped: &DroppedEntry) -> String {
    let at = |t: DateTime<Utc>| t.format("%Y-%m-%d %H:%M:%S UTC");
    let summary = &dropped.summary;
    let kept = match dropped.kept {
        Kept::Seen => panic_record::KEPT_SEEN,
        Kept::Unshown | Kept::ThisRun => panic_record::KEPT_UNSHOWN,
    };
    let when = match (summary.ended_between(), summary.undated()) {
        (Some((first, last)), 0) => format!("between {} and {}", at(first), at(last)),
        (Some((first, last)), undated) => format!(
            "between {} and {} ({undated} of them at an unknown time)",
            at(first),
            at(last)
        ),
        (None, _) => "at an unknown time".to_string(),
    };
    format!(
        "{} older records were dropped to keep only the newest {kept}; those runs ended {when}. \
         Their messages are gone.",
        summary.count()
    )
}

/// The crash history section, as render-DSL source over [`CrashHistory::rows`].
pub const SECTION_SRC: &str = concat!(
    "column(#{gap: 6}, ",
    "text(\"Crash history\", #{bold: true}), ",
    "text(\"Every crash Holon kept a record of, newest first. Reading them changes nothing.\", \
     #{color: \"muted\"}), ",
    "list(#{gap: 10, item_template: column(#{gap: 2}, ",
    "text(col(\"headline\"), #{bold: true, size: 12.0}), ",
    "text(col(\"body\"), #{size: 12.0}))}))"
);

pub fn render_expr() -> anyhow::Result<RenderExpr> {
    holon_api::render_dsl::parse_render_dsl(SECTION_SRC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_source_parses() {
        render_expr().expect("the crash history section parses");
    }

    fn seed(dir: &Path, n: u64, message: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let record = PanicRecord {
            message: message.to_string(),
            location: format!("site.rs:{n}:1"),
            thread: "main".to_string(),
        };
        std::fs::write(
            dir.join(format!("{n}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }

    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        for entry in walkdir(dir) {
            files.push((entry.clone(), std::fs::read(&entry).unwrap()));
        }
        files.sort();
        files
    }

    fn walkdir(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .flat_map(|e| {
                let path = e.unwrap().path();
                if path.is_dir() {
                    walkdir(&path)
                } else {
                    vec![path]
                }
            })
            .collect()
    }

    #[test]
    fn every_record_reads_in_full_newest_first_and_reading_changes_nothing() {
        let config = tempfile::tempdir().unwrap();
        let dir = config.path();
        let long = format!("first line\n{}", "a long message ".repeat(80));
        seed(&dir.join(panic_record::UNSHOWN_DIR), 1, "unshown one");
        seed(&dir.join(panic_record::UNSHOWN_DIR), 2, &long);
        seed(&dir.join(panic_record::SEEN_DIR), 7, "seen seven");
        std::fs::write(dir.join(panic_record::SEEN_DIR).join("notes.txt"), "x").unwrap();
        std::fs::write(
            dir.join(panic_record::SEEN_DROPPED_FILE),
            r#"{"count":3,"first_ended":"2026-10-01T08:00:00Z","last_ended":"2026-10-01T08:02:00Z"}"#,
        )
        .unwrap();
        let before = snapshot(dir);

        let history = CrashHistory::read(dir);

        assert_eq!(snapshot(dir), before, "reading changes no record file");
        let shown: Vec<(Kept, &str)> = history
            .records
            .iter()
            .map(|e| (e.kept, e.record.message.as_str()))
            .collect();
        assert_eq!(
            shown,
            vec![
                (Kept::Unshown, long.as_str()),
                (Kept::Unshown, "unshown one"),
                (Kept::Seen, "seen seven"),
            ]
        );
        let text = history.to_text();
        assert!(text.contains(&long), "the export holds the full message");
        assert!(text.contains("notes.txt"), "a stray is an entry: {text}");
        assert!(
            text.contains("3 older records were dropped"),
            "the dropped summary is an entry: {text}"
        );
        assert_eq!(history.rows().len(), 5);
    }

    #[test]
    fn an_empty_dir_says_so() {
        let config = tempfile::tempdir().unwrap();
        let history = CrashHistory::read(config.path());
        assert!(history.to_text().contains("No crash records"));
        assert_eq!(history.rows().len(), 1);
    }
}
