use petri_net::Discipline;
use petri_net::MemStore;
use petri_net::testkit::StoreOp;
use petri_net::testkit::TestKinds;
use petri_net::testkit::check_store_laws;
use petri_net::testkit::check_store_laws_on;

type Place = (u8, Discipline);

fn mem(places: &[(u8, Discipline)]) -> MemStore<TestKinds> {
    MemStore::new(places.iter().copied(), 7)
}

#[test]
fn mem_store_obeys_the_store_laws() {
    let single_and_fifo = [(0, Discipline::Single), (1, Discipline::Fifo)];
    let fixed: [(&[Place], Vec<StoreOp>); 2] = [
        (
            &single_and_fifo,
            vec![
                StoreOp::Write((0, 0)),
                StoreOp::Bounce {
                    from: (0, 0),
                    to: (1, 2),
                },
            ],
        ),
        (
            &single_and_fifo,
            vec![
                StoreOp::Write((1, 0)),
                StoreOp::Write((1, 0)),
                StoreOp::Bounce {
                    from: (1, 0),
                    to: (1, 1),
                },
            ],
        ),
    ];
    for (places, ops) in fixed {
        check_store_laws_on(&mem, places, &ops).unwrap();
    }
    check_store_laws(mem);
}
