//! Probe: do concurrent cross-moves on two Loro peers merge into a cycle?
//! Peer 1 moves A under B while peer 2 moves B under A; both import each other.
//!
//! Run: cargo run -p holon-loro --example cycle_guard_loro_probe

use loro::LoroDoc;
use loro::TreeID;
use loro::TreeParentId;

fn parent(doc: &LoroDoc, node: TreeID) -> TreeParentId {
    doc.get_tree("t").parent(node).expect("node is in the tree")
}

fn main() -> anyhow::Result<()> {
    let p1 = LoroDoc::new();
    p1.set_peer_id(1)?;
    let t1 = p1.get_tree("t");
    let root = t1.create(None)?;
    let a = t1.create(root)?;
    let b = t1.create(root)?;
    p1.commit();

    let p2 = LoroDoc::new();
    p2.set_peer_id(2)?;
    p2.import(&p1.export(loro::ExportMode::all_updates())?)?;

    t1.mov(a, b)?;
    p1.commit();
    p2.get_tree("t").mov(b, a)?;
    p2.commit();

    let u1 = p1.export(loro::ExportMode::all_updates())?;
    let u2 = p2.export(loro::ExportMode::all_updates())?;
    p1.import(&u2)?;
    p2.import(&u1)?;

    for (name, doc) in [("peer1", &p1), ("peer2", &p2)] {
        let (pa, pb) = (parent(doc, a), parent(doc, b));
        let cycle = pa == TreeParentId::Node(b) && pb == TreeParentId::Node(a);
        println!("{name}: parent(A)={pa:?} parent(B)={pb:?} cycle={cycle}");
        assert!(!cycle, "{name}: merged Loro tree holds a parent cycle");
    }
    assert_eq!(
        (parent(&p1, a), parent(&p1, b)),
        (parent(&p2, a), parent(&p2, b)),
        "peers did not converge"
    );
    println!("OK: merge is acyclic and converged");
    Ok(())
}
