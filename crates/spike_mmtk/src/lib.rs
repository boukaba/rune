use std::sync::OnceLock;
use mmtk::vm::slot::SimpleSlot;
use mmtk::vm::VMBinding;
use mmtk::MMTK;

pub mod object_model;
pub mod scanning;
pub mod collection;
pub mod active_plan;
pub mod reference_glue;

#[derive(Default)]
pub struct RuneVM;

impl VMBinding for RuneVM {
    type VMObjectModel = object_model::RuneObjectModel;
    type VMScanning = scanning::RuneScanning;
    type VMCollection = collection::RuneCollection;
    type VMActivePlan = active_plan::RuneActivePlan;
    type VMReferenceGlue = reference_glue::RuneReferenceGlue;
    type VMSlot = SimpleSlot;
    type VMMemorySlice = mmtk::vm::slot::UnimplementedMemorySlice;
    const MAX_ALIGNMENT: usize = 1 << 6;
}

pub static SINGLETON: OnceLock<Box<MMTK<RuneVM>>> = OnceLock::new();

pub fn mmtk() -> &'static MMTK<RuneVM> {
    SINGLETON.get().expect("MMTK not initialized")
}
