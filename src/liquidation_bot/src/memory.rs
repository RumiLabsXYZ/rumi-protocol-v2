use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::DefaultMemoryImpl;
use std::cell::RefCell;

pub type Mem = VirtualMemory<DefaultMemoryImpl>;

pub const MEM_ID_CONFIG: MemoryId = MemoryId::new(0);
pub const MEM_ID_HISTORY: MemoryId = MemoryId::new(1);
pub const MEM_ID_NEXT_ID: MemoryId = MemoryId::new(2);

thread_local! {
    static MEMORY_MANAGER: RefCell<Option<MemoryManager<DefaultMemoryImpl>>> =
        RefCell::new(None);
}

/// Explicitly initialize the MemoryManager. Must be called AFTER any legacy
/// stable memory rescue, because MemoryManager::init writes its own header
/// at offset 0 on first use (on subsequent calls it reads the existing header
/// and is non-destructive/idempotent).
pub fn init_memory_manager() {
    MEMORY_MANAGER.with(|mm| {
        *mm.borrow_mut() = Some(MemoryManager::init(DefaultMemoryImpl::default()));
    });
}

pub fn get_memory(id: MemoryId) -> Mem {
    MEMORY_MANAGER.with(|mm| {
        mm.borrow()
            .as_ref()
            .expect("MemoryManager not initialized. Call init_memory_manager() first.")
            .get(id)
    })
}
