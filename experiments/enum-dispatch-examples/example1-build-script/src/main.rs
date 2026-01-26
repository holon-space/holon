//! Example 1: Build-script auto-generated enum + dispatch.
//!
//! ## Architecture
//!
//! - `TransitionHandler` trait is defined here
//! - `build.rs` scans `src/transitions/` for `#[transition]` structs
//! - Generates `E2ETransition` enum + `impl TransitionHandler` dispatch
//! - The generated code is included via `include!` from the build output
//!
//! ## How to add a new transition
//!
//! 1. Create `src/transitions/my_new_transition.rs`
//! 2. Add `#[transition]` above `pub struct MyTransition { ... }`
//! 3. Implement `TransitionHandler` for the struct
//! 4. Register the module in `src/transitions/mod.rs`
//!
//! That's it! The enum and dispatch are auto-generated. The generated
//! code is committed to version control (or rebuilt by build.rs).

mod transitions;

// Include the auto-generated enum + dispatch impl
include!(concat!(env!("OUT_DIR"), "/generated_transitions.rs"));

// ── The trait ────────────────────────────────────────────────────────
pub trait TransitionHandler {
    fn apply(&self, ctx: &mut AppContext) -> anyhow::Result<()>;
    fn description(&self) -> &'static str;
}

// ── Mock system-under-test state ─────────────────────────────────────
pub struct AppContext {
    pub db_state: Vec<String>,
    pub file_system: std::collections::HashMap<String, String>,
}

impl AppContext {
    pub fn new() -> Self {
        Self {
            db_state: Vec::new(),
            file_system: std::collections::HashMap::new(),
        }
    }
}

fn main() -> anyhow::Result<()> {
    use transitions::append_block::AppendBlock;
    use transitions::create_document::CreateDocument;
    use transitions::write_org_file::WriteOrgFile;

    let mut ctx = AppContext::new();

    let t1 = E2ETransition::WriteOrgFile(WriteOrgFile {
        filename: "notes.org".into(),
        content: "* TODO Buy milk".into(),
    });
    t1.apply(&mut ctx)?;
    println!(
        "After WriteOrgFile: {:?}",
        ctx.file_system.keys().collect::<Vec<_>>()
    );

    let t2 = E2ETransition::CreateDocument(CreateDocument {
        file_name: "projects.org".into(),
        title: "Projects".into(),
    });
    t2.apply(&mut ctx)?;
    println!(
        "After CreateDocument: {:?}",
        ctx.file_system.keys().collect::<Vec<_>>()
    );

    let t3 = E2ETransition::AppendBlock(AppendBlock {
        doc_uri: "doc:projects".into(),
        content: "* TODO Design API".into(),
    });
    t3.apply(&mut ctx)?;
    println!("After AppendBlock: {:?}", ctx.db_state);

    let transitions: [E2ETransition; 3] = [t1, t2, t3];
    for t in &transitions {
        println!("  → {}", t.description());
    }

    Ok(())
}
