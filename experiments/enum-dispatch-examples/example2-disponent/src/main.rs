//! Example 2: `disponent` — automatic enum + trait dispatch.
//!
//! `disponent::declare!` generates the enum, the trait, and the
//! `impl Trait for Enum` dispatch all at once. Adds async support.
//!
//! ## How to add a new transition
//!
//! 1. Create `src/transitions/my_new_transition.rs`
//! 2. Define a struct and `impl TransitionHandler for MyStruct`
//! 3. Add `MyStruct(MyStruct)` to the enum in `src/transitions.rs`
//! 4. Register module in `src/transitions/mod.rs`
//!
//! One line added to the enum definition file. Dispatch is automatic.

mod transitions;

// ── Mock system-under-test state ─────────────────────────────────────
pub struct AppContext {
    pub db_state: Vec<String>,
    pub file_system: std::collections::HashMap<String, String>,
}

impl AppContext {
    pub fn new() -> Self {
        Self { db_state: Vec::new(), file_system: std::collections::HashMap::new() }
    }
}

fn main() -> anyhow::Result<()> {
    use transitions::E2ETransition;
    use transitions::TransitionHandler;
    use transitions::{append_block::AppendBlock, create_document::CreateDocument,
        write_org_file::WriteOrgFile};

    let mut ctx = AppContext::new();

    let t1 = E2ETransition::WriteOrgFile(WriteOrgFile {
        filename: "notes.org".into(), content: "* TODO Buy milk".into(),
    });
    t1.apply(&mut ctx)?;
    println!("After WriteOrgFile: {:?}", ctx.file_system.keys().collect::<Vec<_>>());

    let t2 = E2ETransition::CreateDocument(CreateDocument {
        file_name: "projects.org".into(), title: "Projects".into(),
    });
    t2.apply(&mut ctx)?;
    println!("After CreateDocument: {:?}", ctx.file_system.keys().collect::<Vec<_>>());

    let t3 = E2ETransition::AppendBlock(AppendBlock {
        doc_uri: "doc:projects".into(), content: "* TODO Design API".into(),
    });
    t3.apply(&mut ctx)?;
    println!("After AppendBlock: {:?}", ctx.db_state);

    let transitions: [E2ETransition; 3] = [t1, t2, t3];
    for t in &transitions {
        println!("  → {}", t.description());
    }

    Ok(())
}
