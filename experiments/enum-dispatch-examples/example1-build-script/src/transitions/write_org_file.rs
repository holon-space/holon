//! Write an org-mode file to the filesystem.

use crate::{AppContext, TransitionHandler};

pub struct WriteOrgFile {
    pub filename: String,
    pub content: String,
}

impl TransitionHandler for WriteOrgFile {
    fn apply(&self, ctx: &mut AppContext) -> anyhow::Result<()> {
        ctx.file_system
            .insert(self.filename.clone(), self.content.clone());
        eprintln!(
            "[WriteOrgFile] Wrote {} ({} bytes)",
            self.filename,
            self.content.len()
        );
        Ok(())
    }

    fn description(&self) -> &'static str {
        "WriteOrgFile"
    }
}
