use crate::RuneVM;
use mmtk::Mutator;
use mmtk::util::opaque_pointer::*;
use mmtk::vm::{Collection, GCThreadContext};

pub struct RuneCollection;

impl Collection<RuneVM> for RuneCollection {
    fn stop_all_mutators<F>(_tls: VMWorkerThread, _mutator_visitor: F)
    where
        F: FnMut(&'static mut Mutator<RuneVM>),
    {
    }

    fn resume_mutators(_tls: VMWorkerThread) {}

    fn block_for_gc(_tls: VMMutatorThread) {}

    fn spawn_gc_thread(_tls: VMThread, _ctx: GCThreadContext<RuneVM>) {}
}
