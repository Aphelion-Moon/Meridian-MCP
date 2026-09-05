//! Opt-in UCRT allocation attribution for a qualified BYOND version.
// SPDX-License-Identifier: MIT
use auxtools::{raw_types, raw_types::procs::ProcId, shutdown, Proc};
use retour::RawDetour;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    cell::UnsafeCell,
    collections::BTreeMap,
    ffi::c_void,
    io,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::{Duration, Instant},
};
use winapi::um::{
    libloaderapi::{GetModuleHandleA, GetProcAddress},
    processthreadsapi::GetCurrentThreadId,
};

#[path = "accounting.rs"]
mod accounting;
use accounting::Accounting;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static THREAD: AtomicU32 = AtomicU32::new(0);
// These values are only accessed on THREAD. Hooks on other threads only call
// the immutable trampolines. Metadata is preallocated before ACTIVE is set.
static mut IN_HOOK: bool = false;
static mut STATE: UnsafeCell<Option<Capture>> = UnsafeCell::new(None);
static mut HOOKS: Option<Vec<RawDetour>> = None;
static mut MALLOC: Option<unsafe extern "C" fn(usize) -> *mut c_void> = None;
static mut CALLOC: Option<unsafe extern "C" fn(usize, usize) -> *mut c_void> = None;
static mut REALLOC: Option<unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void> = None;
static mut FREE: Option<unsafe extern "C" fn(*mut c_void)> = None;

struct Capture {
    accounting: Accounting,
    start: Instant,
    duration: Duration,
    stopped_ms: Option<u64>,
    reason: &'static str,
}
impl Capture {
    fn tick(&mut self) -> bool {
        if self.stopped_ms.is_some() {
            return false;
        }
        if self.start.elapsed() >= self.duration {
            self.stopped_ms = Some(self.duration.as_millis() as u64);
            self.reason = "deadline";
            ACTIVE.store(false, Ordering::Release);
            return false;
        }
        true
    }
}

unsafe fn capability() -> Result<[*const c_void; 4], String> {
    if (
        auxtools::version::BYOND_VERSION_MAJOR,
        auxtools::version::BYOND_VERSION_MINOR,
    ) != (516, 1687)
    {
        return Err("Native memory capture is qualified only for Windows BYOND 516.1687".into());
    }
    let module = GetModuleHandleA(b"ucrtbase.dll\0".as_ptr().cast());
    if module.is_null() {
        return Err("ucrtbase.dll is not loaded; no allocation hooks installed".into());
    }
    let mut addresses: [*const c_void; 4] = [std::ptr::null(); 4];
    for (index, name) in [b"malloc\0".as_slice(), b"calloc\0", b"realloc\0", b"free\0"]
        .iter()
        .enumerate()
    {
        addresses[index] = GetProcAddress(module, name.as_ptr().cast()).cast();
        if addresses[index].is_null() {
            return Err("Required UCRT allocator export unavailable".into());
        }
    }
    Ok(addresses)
}

unsafe fn setup() -> Result<(), String> {
    if HOOKS.is_some() {
        return Ok(());
    }
    let targets = capability()?;
    let replacements = [
        malloc_hook as *const (),
        calloc_hook as *const (),
        realloc_hook as *const (),
        free_hook as *const (),
    ];
    let mut hooks = Vec::with_capacity(4);
    for (target, replacement) in targets.into_iter().zip(replacements) {
        hooks.push(RawDetour::new(target.cast(), replacement.cast()).map_err(|e| e.to_string())?);
    }
    MALLOC = Some(std::mem::transmute(hooks[0].trampoline()));
    CALLOC = Some(std::mem::transmute(hooks[1].trampoline()));
    REALLOC = Some(std::mem::transmute(hooks[2].trampoline()));
    FREE = Some(std::mem::transmute(hooks[3].trampoline()));
    // Publish immutable trampolines before enabling any process-wide hook.
    // Retain every trampoline until process exit, including partial setup: an
    // allocator thread may still be returning through it after disable.
    HOOKS = Some(hooks);
    for hook in HOOKS.as_ref().unwrap() {
        if let Err(error) = hook.enable() {
            // Fail closed for all subsequent capture attempts too.
            SETUP_FAILED.store(true, Ordering::Release);
            return Err(format!("Allocator hook setup failed: {error}"));
        }
    }
    Ok(())
}
static SETUP_FAILED: AtomicBool = AtomicBool::new(false);

unsafe fn enter() -> bool {
    if !ACTIVE.load(Ordering::Acquire) || THREAD.load(Ordering::Relaxed) != GetCurrentThreadId() {
        return false;
    }
    if IN_HOOK {
        return false;
    }
    IN_HOOK = true;
    true
}
unsafe fn current_proc() -> Option<u32> {
    let ctx = *raw_types::funcs::CURRENT_EXECUTION_CONTEXT;
    if ctx.is_null() {
        return None;
    }
    let instance = *(*ctx).proc_instance();
    if instance.is_null() {
        None
    } else {
        Some((*instance).proc.0)
    }
}
unsafe fn record(operation: impl FnOnce(&mut Accounting)) {
    if let Some(capture) = STATE.get_mut() {
        if capture.tick() {
            operation(&mut capture.accounting);
            if capture.accounting.capacity_exceeded {
                capture.reason = "record_limit";
                capture.stopped_ms = Some(capture.start.elapsed().as_millis() as u64);
                ACTIVE.store(false, Ordering::Release);
            }
        }
    }
    IN_HOOK = false;
}
unsafe extern "C" fn malloc_hook(size: usize) -> *mut c_void {
    let entered = enter();
    let proc_id = if entered { current_proc() } else { None };
    let pointer = MALLOC.unwrap()(size);
    if entered {
        record(|a| a.allocate(pointer as usize, size, proc_id));
    }
    pointer
}
unsafe extern "C" fn calloc_hook(count: usize, size: usize) -> *mut c_void {
    let entered = enter();
    let proc_id = if entered { current_proc() } else { None };
    let pointer = CALLOC.unwrap()(count, size);
    if entered {
        record(|a| {
            if let Some(bytes) = count.checked_mul(size) {
                a.allocate(pointer as usize, bytes, proc_id);
            }
        });
    }
    pointer
}
unsafe extern "C" fn realloc_hook(old: *mut c_void, size: usize) -> *mut c_void {
    let entered = enter();
    let proc_id = if entered { current_proc() } else { None };
    let pointer = REALLOC.unwrap()(old, size);
    if entered {
        record(|a| a.reallocate(old as usize, pointer as usize, size, proc_id));
    }
    pointer
}
unsafe extern "C" fn free_hook(pointer: *mut c_void) {
    let entered = enter();
    FREE.unwrap()(pointer);
    if entered {
        record(|a| a.free(pointer as usize));
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    action: String,
    duration_ms: Option<u64>,
    max_records: Option<usize>,
    row_limit: Option<usize>,
}

pub fn command(input: &str) -> String {
    fn run(input: &str) -> Result<Value, String> {
        let request: Request = serde_json::from_str(input).map_err(|e| e.to_string())?;
        let duration = request.duration_ms.unwrap_or(10_000);
        let records = request.max_records.unwrap_or(20_000);
        let rows = request.row_limit.unwrap_or(100);
        if !(1..=60_000).contains(&duration)
            || !(1..=100_000).contains(&records)
            || !(1..=1000).contains(&rows)
        {
            return Err(
                "Capture bounds: duration_ms 1..60000, max_records 1..100000, row_limit 1..1000"
                    .into(),
            );
        }
        unsafe {
            if SETUP_FAILED.load(Ordering::Acquire) {
                return Err(
                    "Allocator hook setup previously failed; restart the owned debugger".into(),
                );
            }
            match request.action.as_str() {
                "status" => {
                    capability()?;
                    if let Some(capture) = STATE.get_mut() {
                        capture.tick();
                    }
                    Ok(
                        json!({"available":true,"pending_capture":STATE.get_mut().is_some(),"recording":ACTIVE.load(Ordering::Acquire)}),
                    )
                }
                "start" => {
                    if STATE.get_mut().is_some() {
                        return Err("A capture is pending; stop it before starting another".into());
                    }
                    capability()?;
                    let capture = Capture {
                        accounting: Accounting::new(records),
                        start: Instant::now(),
                        duration: Duration::from_millis(duration),
                        stopped_ms: None,
                        reason: "requested",
                    };
                    setup()?;
                    THREAD.store(GetCurrentThreadId(), Ordering::Relaxed);
                    *STATE.get_mut() = Some(Capture {
                        start: Instant::now(),
                        ..capture
                    });
                    ACTIVE.store(true, Ordering::Release);
                    Ok(json!({"recording":true,"duration_ms":duration,"max_records":records}))
                }
                "stop" => {
                    ACTIVE.store(false, Ordering::Release);
                    let mut capture = STATE.get_mut().take().ok_or("No capture is pending")?;
                    capture.tick();
                    let elapsed = capture
                        .stopped_ms
                        .unwrap_or(capture.start.elapsed().as_millis() as u64);
                    let mut totals: BTreeMap<u32, (u64, u64)> = BTreeMap::new();
                    for allocation in capture.accounting.live.values() {
                        let total = totals.entry(allocation.proc_id).or_default();
                        total.0 += allocation.size as u64;
                        total.1 += 1;
                    }
                    let total_rows = totals.len();
                    let mut totals: Vec<_> = totals.into_iter().collect();
                    totals.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(&b.0)));
                    let values: Vec<_> = totals.into_iter().take(rows).map(|(id,(bytes,count))| {
                        let mut truncated = false;
                        let path = Proc::from_id(ProcId(id)).map(|p| {
                            let mut path = p.path;
                            if path.len() > 256 {
                                let mut end = 256;
                                while !path.is_char_boundary(end) { end -= 1; }
                                path.truncate(end);
                                truncated = true;
                            }
                            path
                        });
                        json!({"proc_id":id,"proc_path":path,"proc_path_truncated":truncated,"outstanding_requested_bytes":bytes,"allocation_count":count})
                    }).collect();
                    Ok(
                        json!({"recording":false,"stop_reason":capture.reason,"elapsed_ms":elapsed,
                        "capacity_exceeded":capture.accounting.capacity_exceeded,"total_procedures":total_rows,"rows_truncated":total_rows > rows,
                        "attributed_allocation_calls":capture.accounting.attributed_calls,"unattributed_allocation_calls":capture.accounting.unattributed_calls,
                        "outstanding_requested_bytes":capture.accounting.live_bytes,"peak_outstanding_requested_bytes":capture.accounting.peak_bytes,
                        "procedures":values}),
                    )
                }
                _ => Err("Expected status, start or stop".into()),
            }
        }
    }
    let result = match run(input) {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":error}),
    };
    json!({"protocol_version":1,"allocator":"ucrtbase.dll","scope":"DM-attributed allocations and frees observed on the VM thread",
        "excludes":["other threads", "preexisting allocations", "custom allocators and VM pools", "object identities and retaining references"],
        "evidence":result}).to_string()
}

// Remove the upstream unbounded file-writing command from this helper.
pub fn begin(_: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Use dm_debug_memory",
    ))
}
pub fn end() {}
#[shutdown]
fn shutdown() {
    ACTIVE.store(false, Ordering::Release);
    unsafe {
        STATE.get_mut().take();
    }
}
