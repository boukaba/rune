use crate::RuneVM;
use mmtk::util::ObjectReference;
use mmtk::util::opaque_pointer::VMWorkerThread;
use mmtk::vm::ReferenceGlue;

pub struct RuneReferenceGlue;

impl ReferenceGlue<RuneVM> for RuneReferenceGlue {
    type FinalizableType = ObjectReference;

    fn set_referent(_reference: ObjectReference, _referent: ObjectReference) {}
    fn get_referent(_object: ObjectReference) -> Option<ObjectReference> {
        None
    }
    fn clear_referent(_object: ObjectReference) {}
    fn enqueue_references(_references: &[ObjectReference], _tls: VMWorkerThread) {}
}
