//! `inv-crash-restore-copies-match-ref` — a file that boots equal to its merge
//! base while its write-back was owed keeps its bytes in a conflict copy.
//!
//! @pbt oracle model-equivalence — beside every file the model judges lie
//!   exactly as many conflict copies as its crash-restores left, each holding
//!   the texts of the merge base it saved
//! @pbt covers crash-restore-ambiguous — the store wins the restore, so the
//!   copy is the only place the file's version from before the save survives
//! @pbt slips-if-removed the boot keeps the store's version and overwrites the
//!   file without saving its bytes first, or saves a copy of a file the user
//!   edited while the app was down

use holon_pbt_core::capabilities::RefCopies;
use holon_pbt_core::capabilities::SutEditorSaves;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvCrashRestoreCopiesMatchRef;

impl InvCrashRestoreCopiesMatchRef {
    pub const ID: InvariantId = InvariantId("inv-crash-restore-copies-match-ref");
}

/// The file a conflict copy named `<stem>.conflict-<stamp>-<seq>.<ext>` saves.
fn copied_file(copy: &str) -> String {
    let (stem, ext) = copy
        .rsplit_once('.')
        .unwrap_or_else(|| panic!("conflict copy {copy} has no extension"));
    let at = stem
        .rfind(".conflict-")
        .unwrap_or_else(|| panic!("conflict copy {copy} has no conflict marker"));
    format!("{}.{ext}", &stem[..at])
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvCrashRestoreCopiesMatchRef
where
    R: RefCopies,
    S: SutEditorSaves,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, reference: &R, sut: &S) -> InvariantResult {
        let judged = reference.crash_restore_copies();
        if judged.is_empty() {
            return InvariantResult::Ok;
        }
        let on_disk = sut.conflict_copies().await;
        let mut failures = Vec::new();
        for (file, expected) in &judged {
            let copies: Vec<&(String, String)> = on_disk
                .iter()
                .filter(|(copy, _)| copied_file(copy) == *file)
                .collect();
            let unmatched: Vec<&Vec<String>> = expected
                .iter()
                .filter(|texts| {
                    !copies.iter().any(|(_, text)| {
                        texts
                            .iter()
                            .flat_map(|t| t.lines())
                            .filter(|line| !line.trim().is_empty())
                            .all(|line| text.contains(line))
                    })
                })
                .collect();
            if copies.len() != expected.len() || !unmatched.is_empty() {
                failures.push(format!(
                    "{file} has {} conflict copies where the model expects {}; no copy holds \
                     the merge base texts {unmatched:?}. Copies: {copies:?}",
                    copies.len(),
                    expected.len(),
                ));
            }
        }
        if failures.is_empty() {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(failures.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::copied_file;

    #[test]
    fn a_copy_names_the_file_it_saves() {
        assert_eq!(
            copied_file("hm4.conflict-20261024T101500Z-000.org"),
            "hm4.org"
        );
        assert_eq!(
            copied_file("a.conflict-b.conflict-20261024T101500Z-001.org"),
            "a.conflict-b.org"
        );
    }
}
