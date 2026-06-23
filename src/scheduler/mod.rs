/// ferrox/src/scheduler/mod.rs
/// Async cooperative task scheduler with per-hart run queues
use alloc::collections::VecDeque;
use alloc::boxed::Box;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

static TASK_ID_CTR: AtomicU64 = AtomicU64::new(1);

pub struct Task {
    pub id:     u64,
    pub name:   &'static str,
    future:     Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl Task {
    pub fn new(name: &'static str, f: impl Future<Output = ()> + Send + 'static) -> Self {
        Task {
            id:     TASK_ID_CTR.fetch_add(1, Ordering::Relaxed),
            name,
            future: Box::pin(f),
        }
    }

    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.future.as_mut().poll(cx)
    }
}

// ── Per-hart executor ────────────────────────────────────────────────────────
pub struct Executor {
    queue: VecDeque<Task>,
}

impl Executor {
    pub const fn new() -> Self {
        Executor { queue: VecDeque::new() }
    }

    pub fn spawn(&mut self, task: Task) {
        kprintln!("[sched] spawn  task={} id={}", task.name, task.id);
        self.queue.push_back(task);
    }

    pub fn run_once(&mut self) {
        let waker  = dummy_waker();
        let mut cx = Context::from_waker(&waker);

        // Round-robin over all tasks
        let mut pending = VecDeque::new();
        while let Some(mut task) = self.queue.pop_front() {
            match task.poll(&mut cx) {
                Poll::Ready(()) => {
                    kprintln!("[sched] done   task={} id={}", task.name, task.id);
                }
                Poll::Pending => {
                    pending.push_back(task);
                }
            }
        }
        self.queue = pending;
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

// ── Global per-hart executors (up to 8 harts) ────────────────────────────────
const MAX_HARTS: usize = 8;
static mut EXECUTORS: [Option<Executor>; MAX_HARTS] = [
    None, None, None, None, None, None, None, None,
];

pub fn run(hartid: usize) -> ! {
    let exec = unsafe {
        EXECUTORS[hartid].get_or_insert(Executor::new())
    };

    // Spawn built-in kernel tasks on hart 0
    if hartid == 0 {
        exec.spawn(Task::new("heartbeat",  heartbeat_task()));
        exec.spawn(Task::new("irq_pump",   irq_pump_task()));
    }

    loop {
        exec.run_once();
        // WFI if no tasks are ready (interrupt will reschedule)
        if exec.is_empty() {
            unsafe { core::arch::asm!("wfi") };
        }
    }
}

/// Called from trap_handler on timer interrupt
pub fn tick() {
    // Future: update task timers, wake sleeping tasks
}

// ── Built-in kernel tasks ─────────────────────────────────────────────────────
async fn heartbeat_task() {
    let mut count = 0u64;
    loop {
        count += 1;
        kprintln!("[heartbeat] tick {}", count);
        // Yield to other tasks
        YieldNow(false).await;
        // In real kernel: sleep for ~1s via timer future
    }
}

async fn irq_pump_task() {
    loop {
        // Poll any deferred IRQ work (device drivers post here)
        YieldNow(false).await;
    }
}

// ── Minimal yield future ─────────────────────────────────────────────────────
struct YieldNow(bool);
impl Future for YieldNow {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

// ── Dummy waker (no heap allocation needed in no_std context) ────────────────
fn dummy_waker() -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |p| RawWaker::new(p, &VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
}
