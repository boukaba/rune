/// Write barrier interface.
/// For MarkSweep (Phase 1), barriers are no-ops.
/// GenImmix (Phase 7) will use a card-table barrier.
pub trait WriteBarrier {
    fn pre_write(&self, _obj: *mut u8, _offset: usize) {}
    fn post_write(&self, _obj: *mut u8, _offset: usize) {}
}

pub struct NoOpBarrier;

impl WriteBarrier for NoOpBarrier {}
