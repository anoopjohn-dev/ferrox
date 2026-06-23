/// ferrox/src/mm/mod.rs
/// Physical memory manager + Sv39 page-table allocator
use core::sync::atomic::{AtomicUsize, Ordering};

// ── Physical frame allocator (bitmap-based) ──────────────────────────────────
const PAGE_SIZE: usize = 4096;
const MAX_PAGES: usize = 65536;  // 256 MB / 4 KB

static FREE_PAGES: AtomicUsize = AtomicUsize::new(0);

// Bitmap: 1 bit per frame. 1 = free. Stored in BSS.
static mut FRAME_BITMAP: [u64; MAX_PAGES / 64] = [0u64; MAX_PAGES / 64];
static mut PHYS_BASE: usize = 0;

pub fn init(dtb_ptr: usize) {
    // Parse device tree to find RAM regions (simplified: assume 0x80000000 + 256MB)
    let ram_start = 0x8000_0000usize;
    let ram_end   = ram_start + 256 * 1024 * 1024;

    // Reserve first 4 MB for kernel image + stack
    let usable_start = ram_start + 4 * 1024 * 1024;
    let num_frames   = (ram_end - usable_start) / PAGE_SIZE;

    unsafe {
        PHYS_BASE = usable_start;
        // Mark all frames as free
        let words = num_frames / 64;
        for i in 0..words {
            FRAME_BITMAP[i] = !0u64;
        }
        FREE_PAGES.store(num_frames, Ordering::Relaxed);
    }

    kprintln!("[mm] init  base={:#x}  frames={}", usable_start, num_frames);
    let _ = dtb_ptr; // TODO: parse memory regions from DTB
}

/// Allocate a physical frame. Returns physical address or None.
pub fn alloc_frame() -> Option<usize> {
    unsafe {
        for (word_idx, word) in FRAME_BITMAP.iter_mut().enumerate() {
            if *word != 0 {
                let bit = word.trailing_zeros() as usize;
                *word &= !( 1u64 << bit );
                FREE_PAGES.fetch_sub(1, Ordering::Relaxed);
                return Some(PHYS_BASE + (word_idx * 64 + bit) * PAGE_SIZE);
            }
        }
    }
    None
}

/// Free a physical frame.
pub fn free_frame(phys: usize) {
    unsafe {
        let frame = (phys - PHYS_BASE) / PAGE_SIZE;
        FRAME_BITMAP[frame / 64] |= 1u64 << (frame % 64);
        FREE_PAGES.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn free_frame_count() -> usize {
    FREE_PAGES.load(Ordering::Relaxed)
}

// ── Sv39 page-table entry ────────────────────────────────────────────────────
#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct Pte(u64);

impl Pte {
    pub const V: u64 = 1 << 0; // Valid
    pub const R: u64 = 1 << 1; // Read
    pub const W: u64 = 1 << 2; // Write
    pub const X: u64 = 1 << 3; // Execute
    pub const U: u64 = 1 << 4; // User-accessible
    pub const G: u64 = 1 << 5; // Global
    pub const A: u64 = 1 << 6; // Accessed
    pub const D: u64 = 1 << 7; // Dirty

    pub fn new(ppn: usize, flags: u64) -> Self {
        Pte(((ppn as u64) << 10) | flags)
    }
    pub fn is_valid(self)  -> bool { self.0 & Self::V != 0 }
    pub fn is_leaf(self)   -> bool { self.0 & (Self::R | Self::W | Self::X) != 0 }
    pub fn ppn(self)       -> usize { (self.0 >> 10) as usize }
    pub fn phys_addr(self) -> usize { self.ppn() * PAGE_SIZE }
}

// ── Page table (Sv39: 512 entries per level) ─────────────────────────────────
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [Pte; 512],
}

impl PageTable {
    /// Map `virt` → `phys` with given flags, allocating intermediate tables.
    pub fn map(&mut self, virt: usize, phys: usize, flags: u64) -> bool {
        let vpn = [
            (virt >> 12)  & 0x1FF,
            (virt >> 21)  & 0x1FF,
            (virt >> 30)  & 0x1FF,
        ];
        let ppn = phys / PAGE_SIZE;

        let mut table = self as *mut PageTable;
        for level in (1..=2).rev() {
            let entry = unsafe { &mut (*table).entries[vpn[level]] };
            if !entry.is_valid() {
                let new_page = alloc_frame()?;
                *entry = Pte::new(new_page / PAGE_SIZE, Pte::V);
            }
            table = unsafe { &mut *(entry.phys_addr() as *mut PageTable) };
        }
        unsafe { (*table).entries[vpn[0]] = Pte::new(ppn, flags | Pte::V | Pte::A | Pte::D); }
        true
    }

    pub fn satp_value(&self) -> usize {
        // Mode 8 = Sv39
        (8 << 60) | ((self as *const _ as usize) / PAGE_SIZE)
    }
}

fn alloc_frame() -> Option<usize> { super::mm::alloc_frame() }
