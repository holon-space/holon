//! A write-back the controller cannot land is disclosed to the user, naming
//! the file and the reason, and comes back marked disclosed, so the write-back
//! loop has nothing left to only log.
//!
//! One example, not a PBT: the seam is a block whose id no org `:ID:` line can
//! carry, the render's one refusal a double store can reach; the id collision
//! of a pair's conflict copy is driven end to end by the two-instance PBT.

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_filesystem::BlockDelta;
use org_vault_double::DOC_ID;
use org_vault_double::vault;

use crate::org_vault_double;

#[tokio::test]
async fn a_refused_writeback_is_disclosed_with_its_file_and_marked() {
    let page = EntityUri::block(DOC_ID);
    let block = Block::new_text(EntityUri::block(":bad"), page.clone(), "bad");
    let mut v = vault();
    let path = v.path.clone();
    let _ = v.controller.on_file_changed(&path).await.unwrap();
    v.put(block.clone());

    // The holder renders only once it holds every member the store does, so
    // the batch carries the whole document, each block after its sibling.
    let page_block = v.docs.by_id.lock().unwrap()[&page].clone();
    let mut kids: Vec<Block> = v
        .store
        .lock()
        .unwrap()
        .values()
        .filter(|b| b.parent_id == page && b.id != block.id)
        .cloned()
        .collect();
    kids.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    kids.push(block);
    let mut batch = vec![(
        page.clone(),
        BlockDelta::Upsert {
            block: page_block,
            prev: None,
        },
    )];
    let mut prev = None;
    for kid in kids {
        let id = kid.id.clone();
        batch.push((page.clone(), BlockDelta::Upsert { block: kid, prev }));
        prev = Some(id);
    }
    let verdicts = v.controller.on_block_changed_coalesced(&batch).await;

    let [(doc, verdict)] = verdicts.as_slice() else {
        panic!("one document changed, got {verdicts:?}");
    };
    assert_eq!(doc, &page);
    let err = verdict
        .as_ref()
        .expect_err("the render must refuse the file");
    assert!(
        holon_filesystem::already_disclosed(err),
        "the refusal must come back marked disclosed: {err:#}"
    );
    let file = path.display().to_string();
    let calls = v.disclosures.calls();
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with(&format!("lossy {file}: ")) && c.contains("not written")),
        "the refusal must be disclosed naming {file} and the reason: {calls:?}"
    );
}
