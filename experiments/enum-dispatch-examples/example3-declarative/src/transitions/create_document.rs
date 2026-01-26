use crate::AppContext;
use super::TransitionHandler;

pub struct CreateDocument {
    pub file_name: String,
    pub title: String,
}

impl TransitionHandler for CreateDocument {
    fn apply(&self, ctx: &mut AppContext) -> anyhow::Result<()> {
        let uri = format!("doc:{}", self.file_name.trim_end_matches(".org"));
        ctx.file_system.insert(self.file_name.clone(), format!("#+TITLE: {}\n", self.title));
        ctx.db_state.push(format!("created:{}", uri));
        eprintln!("[CreateDocument] Created {} → {}", self.file_name, uri);
        Ok(())
    }
    fn description(&self) -> &'static str { "CreateDocument" }
}
