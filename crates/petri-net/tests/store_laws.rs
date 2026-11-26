use petri_net::testkit::{StoreOp, TestKinds, check_store_laws, check_store_laws_on};
use petri_net::{Discipline, MemStore};

fn mem(places: &[(u8, Discipline)]) -> MemStore<TestKinds> {
    MemStore::new(places.iter().copied(), 7)
}

#[test]
fn mem_store_obeys_the_store_laws() {
    let single_and_fifo = [(0, Discipline::Single), (1, Discipline::Fifo)];
    let fixed: [(&[(u8, Discipline)], Vec<StoreOp>); 2] = [
        (&single_and_fifo, vec![StoreOp::Write((0, 0)), StoreOp::Bounce { from: (0, 0), to: (1, 2) }]),
        (
            &single_and_fifo,
            vec![StoreOp::Write((1, 0)), StoreOp::Write((1, 0)), StoreOp::Bounce { from: (1, 0), to: (1, 1) }],
        ),
    ];
    for (places, ops) in fixed {
        check_store_laws_on(&mem, places, &ops).unwrap();
    }
    check_store_laws(mem);
}
