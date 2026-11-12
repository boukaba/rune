use crate::RuneVM;
use crate::object_model;
use mmtk::Mutator;
use mmtk::util::ObjectReference;
use mmtk::util::opaque_pointer::*;
use mmtk::vm::slot::SimpleSlot;
use mmtk::vm::{RootsWorkFactory, Scanning, SlotVisitor};

pub struct RuneScanning;

impl Scanning<RuneVM> for RuneScanning {
    fn scan_object<SV: SlotVisitor<SimpleSlot>>(
        _tls: VMWorkerThread,
        object: ObjectReference,
        slot_visitor: &mut SV,
    ) {
        unsafe {
            let num_slots = (object_model::object_size(object) - object_model::HEADER_SIZE)
                / object_model::SLOT_SIZE;

            for i in 0..num_slots {
                let slot_val = object_model::get_slot(object, i);
                if slot_val != 0 && (slot_val & 1) == 0 {
                    let slot_addr = object
                        .to_raw_address()
                        .add(object_model::HEADER_SIZE + i * object_model::SLOT_SIZE);
                    let slot = SimpleSlot::from_address(slot_addr);
                    slot_visitor.visit_slot(slot);
                }
            }
        }
    }

    fn notify_initial_thread_scan_complete(_partial_scan: bool, _tls: VMWorkerThread) {}

    fn scan_roots_in_mutator_thread(
        _tls: VMWorkerThread,
        _mutator: &'static mut Mutator<RuneVM>,
        _factory: impl RootsWorkFactory<SimpleSlot>,
    ) {
    }

    fn scan_vm_specific_roots(_tls: VMWorkerThread, _factory: impl RootsWorkFactory<SimpleSlot>) {}

    fn supports_return_barrier() -> bool {
        false
    }

    fn prepare_for_roots_re_scanning() {}
}
