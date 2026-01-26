//! Append a block to an existing document.

use crate::{AppContext, TransitionHandler};

pub struct AppendBlock {
    pub doc_uri: String,
    pub content: String,
}

impl TransitionHandler for AppendBlock {
    fn apply(&self, ctx: &mut AppContext) -> anyhow::Result<()> {
        ctx.db_state
            .push(format!("block:{} → {}", self.doc_uri, self.content));
        eprintln!(
            "[AppendBlock] Appended to {}: {:?}",
            self.doc_uri, self.content
        );
        Ok(())
    }

    fn description(&self) -> &'static str {
        "AppendBlock"
    }
}
