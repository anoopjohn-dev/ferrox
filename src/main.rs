// ferrox — async bare-metal OS kernel for RISC-V
// Entry point: _start → rust_main (called from boot.S)
#![no_std]
#![no_main]
#![feature(asm_const, naked_functions, allocator_api)]

mod hal;
mod mm;
mod scheduler;
mod ipc;
mod drivers;

use core::sync::atomic::{AtomicBool, Ordering};
use hal::{uart, timer, plic};

// ── Per-hart boot flag ───────────────────────────────────────────────────────
static BOOT_DONE: AtomicBool = AtomicBool::new(false);

// ── Kernel entry — hart 0 only ───────────────────────────────────────────────
#[no_mangle]
pub extern "C" fn rust_main(hartid: usize, dtb_ptr: usize) -> ! {
    if hartid == 0 {
        uart::init(115_200);
        kprintln!("ferrox v0.1.0  hart={} dtb={:#x}", hartid, dtb_ptr);

        mm::init(dtb_ptr);
        plic::init();
        timer::set_next_tick(timer::TICK_HZ / 100);  // 100 Hz scheduler tick

        BOOT_DONE.store(true, Ordering::Release);
    } else {
        // Secondary harts spin until primary finishes platform init
        while !BOOT_DONE.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
        kprintln!("hart {} online", hartid);
    }

    scheduler::run(hartid)
}

// ── Trap handler (M-mode) ────────────────────────────────────────────────────
#[no_mangle]
pub extern "C" fn trap_handler(cause: usize, epc: usize, tval: usize) -> usize {
    const TIMER_IRQ: usize = 1 << 63 | 7;
    const EXTERN_IRQ: usize = 1 << 63 | 11;

    match cause {
        TIMER_IRQ => {
            timer::set_next_tick(timer::TICK_HZ / 100);
            scheduler::tick();
            epc
        }
        EXTERN_IRQ => {
            let irq = plic::claim();
            drivers::dispatch_irq(irq);
            plic::complete(irq);
            epc
        }
        _ if cause & (1 << 63) == 0 => {
            // Synchronous exception — handle syscall or fault
            if cause == 8 || cause == 9 || cause == 11 {
                // ecall from U/S/M mode
                ipc::handle_syscall(epc, tval)
            } else {
                panic!("unhandled exception  cause={:#x} epc={:#x} tval={:#x}",
                       cause, epc, tval);
            }
        }
        _ => {
            kprintln!("unknown interrupt cause={:#x}", cause);
            epc
        }
    }
}

// ── Panic handler ────────────────────────────────────────────────────────────
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    kprintln!("\n!!! KERNEL PANIC !!!");
    if let Some(loc) = info.location() {
        kprintln!("  {}:{}", loc.file(), loc.line());
    }
    if let Some(msg) = info.message().as_str() {
        kprintln!("  {}", msg);
    }
    loop { unsafe { core::arch::asm!("wfi") } }
}
