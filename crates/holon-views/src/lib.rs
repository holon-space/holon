//! The incremental view engine: derived views over the authority data, kept
//! by differential dataflow and versioned by the commit clock. The only crate
//! that names `differential_dataflow` or `timely`.

pub mod batch;
pub mod error;
mod eval;
pub mod lower;
pub mod plan;
pub mod row;

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use differential_dataflow::input::Input;
    use timely::WorkerConfig;
    use timely::communication::Allocator;
    use timely::communication::allocator::Thread;
    use timely::dataflow::operators::probe::Handle;
    use timely::worker::Worker;

    /// The engine drives its own worker, so the dataflow steps on a thread
    /// the engine owns.
    #[test]
    fn a_worker_the_engine_owns_counts_children() {
        let mut worker = Worker::new(
            WorkerConfig::default(),
            Allocator::Thread(Thread::default()),
            None,
        );
        let probe = Handle::new();
        let counts = Rc::new(RefCell::new(Vec::new()));
        let sink = counts.clone();
        let mut input = worker.dataflow::<u64, _, _>(|scope| {
            let (input, edges) = scope.new_collection::<(u32, u32), isize>();
            edges
                .map(|(parent, _child)| parent)
                .count()
                .inspect(move |((parent, n), time, diff)| {
                    sink.borrow_mut().push((*parent, *n, *time, *diff))
                })
                .probe_with(&probe);
            input
        });
        input.insert((1, 10));
        input.insert((1, 11));
        input.advance_to(1);
        input.flush();
        worker.step_while(|| probe.less_than(input.time()));
        assert_eq!(*counts.borrow(), vec![(1, 2, 0, 1)]);
    }
}
