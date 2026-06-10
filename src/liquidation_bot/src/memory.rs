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

/// Magic prefix of the ic-stable-structures MemoryManager header at offset 0.
const MEMORY_MANAGER_MAGIC: &[u8; 3] = b"MGR";

/// True when `first_bytes` starts with the MemoryManager header magic.
pub fn is_memory_manager_header(first_bytes: &[u8]) -> bool {
    first_bytes.len() >= MEMORY_MANAGER_MAGIC.len()
        && &first_bytes[..MEMORY_MANAGER_MAGIC.len()] == MEMORY_MANAGER_MAGIC
}

/// UPG-001: true when raw stable memory already holds the MemoryManager
/// layout. Reads the raw magic at offset 0 directly (independent of the heap
/// `migrated_to_stable_structures` flag), so a wiped/defaulted heap state can
/// never re-arm the legacy raw-offset-0 save and clobber the MGR header.
pub fn memory_manager_layout_exists() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        if ic_cdk::api::stable::stable64_size() == 0 {
            return false;
        }
        let mut magic = [0u8; 3];
        ic_cdk::api::stable::stable64_read(0, &mut magic);
        is_memory_manager_header(&magic)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Host builds have no raw ic0 stable memory; an initialized
        // MemoryManager counts as the layout existing so unit tests can
        // exercise the legacy-save fence.
        MEMORY_MANAGER.with(|mm| mm.borrow().is_some())
    }
}
