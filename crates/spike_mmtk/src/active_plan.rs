use crate::RuneVM;
use mmtk::util::opaque_pointer::*;
use mmtk::vm::ActivePlan;
use mmtk::Mutator;

pub struct RuneActivePlan;

impl ActivePlan<RuneVM> for RuneActivePlan {
    fn number_of_mutators() -> usize {
        1
    }

    fn is_mutator(_tls: VMThread) -> bool {
        true
    }

    fn mutator(_tls: VMMutatorThread) -> &'static mut Mutator<RuneVM> {
        unimplemented!()
    }

    fn mutators<'a>() -> Box<dyn Iterator<Item = &'a mut Mutator<RuneVM>> + 'a> {
        Box::new(std::iter::empty())
    }
}
