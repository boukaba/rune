use crate::RuneVM;
use mmtk::util::copy::{CopySemantics, GCWorkerCopyContext};
use mmtk::util::{Address, ObjectReference};
use mmtk::vm::*;
use std::sync::atomic::AtomicU64;

pub const HEADER_SIZE: usize = 8;
pub const SLOT_SIZE: usize = 8;

pub static SHAPE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct RuneObjectModel;

pub fn object_size(_object: ObjectReference) -> usize {
    HEADER_SIZE + 2 * SLOT_SIZE
}

impl ObjectModel<RuneVM> for RuneObjectModel {
    const GLOBAL_LOG_BIT_SPEC: VMGlobalLogBitSpec = VMGlobalLogBitSpec::side_first();

    const LOCAL_FORWARDING_POINTER_SPEC: VMLocalForwardingPointerSpec =
        VMLocalForwardingPointerSpec::side_first();

    const LOCAL_FORWARDING_BITS_SPEC: VMLocalForwardingBitsSpec =
        VMLocalForwardingBitsSpec::side_after(Self::LOCAL_FORWARDING_POINTER_SPEC.as_spec());

    const LOCAL_MARK_BIT_SPEC: VMLocalMarkBitSpec =
        VMLocalMarkBitSpec::side_after(Self::LOCAL_FORWARDING_BITS_SPEC.as_spec());

    const LOCAL_LOS_MARK_NURSERY_SPEC: VMLocalLOSMarkNurserySpec =
        VMLocalLOSMarkNurserySpec::side_after(Self::LOCAL_MARK_BIT_SPEC.as_spec());

    const OBJECT_REF_OFFSET_LOWER_BOUND: isize = 0;

    fn copy(
        _from: ObjectReference,
        _semantics: CopySemantics,
        _copy_context: &mut GCWorkerCopyContext<RuneVM>,
    ) -> ObjectReference {
        panic!("Copy not supported in MarkSweep")
    }

    fn copy_to(_from: ObjectReference, _to: ObjectReference, _region: Address) -> Address {
        panic!("Copy not supported in MarkSweep")
    }

    fn get_current_size(object: ObjectReference) -> usize {
        object_size(object)
    }

    fn get_size_when_copied(object: ObjectReference) -> usize {
        object_size(object)
    }

    fn get_align_when_copied(_object: ObjectReference) -> usize {
        8
    }

    fn get_align_offset_when_copied(_object: ObjectReference) -> usize {
        0
    }

    fn get_reference_when_copied_to(_from: ObjectReference, _to: Address) -> ObjectReference {
        panic!("Copy not supported in MarkSweep")
    }

    fn get_type_descriptor(_reference: ObjectReference) -> &'static [i8] {
        &[]
    }

    fn ref_to_object_start(object: ObjectReference) -> Address {
        object.to_raw_address()
    }

    fn ref_to_header(object: ObjectReference) -> Address {
        object.to_raw_address()
    }

    fn dump_object(_object: ObjectReference) {}
}

pub fn alloc_rune_object(mutator: &mut mmtk::Mutator<RuneVM>, num_slots: usize) -> ObjectReference {
    let size = HEADER_SIZE + num_slots * SLOT_SIZE;
    let align = 8;
    let offset = 0;
    let semantics = mmtk::AllocationSemantics::Default;

    let addr = mmtk::memory_manager::alloc::<RuneVM>(mutator, size, align, offset, semantics);
    assert!(!addr.is_zero(), "MMTk allocation returned null");

    let object =
        ObjectReference::from_raw_address(addr).expect("MMTk alloc returned invalid address");
    mmtk::memory_manager::post_alloc::<RuneVM>(mutator, object, size, semantics);

    object
}

pub unsafe fn set_shape_id(object: ObjectReference, shape_id: u64) {
    unsafe {
        let addr = object.to_raw_address();
        std::ptr::write(addr.to_mut_ptr::<u64>(), shape_id);
    }
}

pub unsafe fn get_shape_id(object: ObjectReference) -> u64 {
    unsafe {
        let addr = object.to_raw_address();
        std::ptr::read(addr.to_ptr::<u64>())
    }
}

pub unsafe fn set_slot(object: ObjectReference, index: usize, value: u64) {
    unsafe {
        let addr = object.to_raw_address();
        let slot_ptr = addr.add(HEADER_SIZE + index * SLOT_SIZE);
        std::ptr::write(slot_ptr.to_mut_ptr::<u64>(), value);
    }
}

pub unsafe fn get_slot(object: ObjectReference, index: usize) -> u64 {
    unsafe {
        let addr = object.to_raw_address();
        let slot_ptr = addr.add(HEADER_SIZE + index * SLOT_SIZE);
        std::ptr::read(slot_ptr.to_ptr::<u64>())
    }
}
