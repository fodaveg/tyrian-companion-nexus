//! Keeps `SuspendThread`, `GetThreadContext`, `SetThreadContext`, `ResumeThread` and
//! `SetThreadPriority` out of this DLL's import table.
//!
//! No code in this addon calls any of them. They used to be imported anyway, because of how
//! the mingw runtime is linked in (see `.cargo/config.toml`): libstdc++ (`guard.o`,
//! `eh_alloc.o`) and libgcc (`emutls.o`) need `pthread_once`, `pthread_key_create`,
//! `pthread_getspecific`, `pthread_setspecific` and the mutex and condition calls, which pulls
//! `libpthread.a(libwinpthread_la-thread.o)` and `(libwinpthread_la-sched.o)` into the link.
//! Fedora builds winpthreads without `-ffunction-sections`: each member is one `.text`
//! section, so `--gc-sections` keeps every function of a member as long as one is used. Three
//! of the functions kept that way are the only code in the whole link that references these
//! five imports, and nothing reaches any of the three:
//!
//! - `pthread_cancel` (`SuspendThread`, `GetThreadContext`, `SetThreadContext`,
//!   `ResumeThread`): its only caller is `pthread_kill`, which nothing references;
//! - `pthread_create` (`ResumeThread`, `SetThreadPriority`): nothing references it; the
//!   threads of this DLL are Rust's, made with `CreateThread`;
//! - `pthread_setschedparam` (`SetThreadPriority`): nothing references it.
//!
//! Measured on the linked DLL, not assumed: `-Wl,-Map=...,--cref` for who references what
//! across objects, and its disassembly and data for any call, jump, address or stored pointer
//! to those functions. `scripts/check-dead-thread-code.sh` repeats that measurement and has to
//! pass on every release build: a new winpthreads or libstdc++, or a C++ dependency that
//! starts a thread with `pthread_create`, would still link and still import none of the five,
//! and that `pthread_create` would report success for a thread these stand-ins never resume.
//! `GetThreadPriority` is not here: winpthreads' own bookkeeping of the
//! calling thread (`__pthread_self_lite`) uses it, and that path is live.
//!
//! An import only enters the table when the linker has to pull the import library's member
//! to resolve `__imp_<name>`. Defining those symbols here resolves them first, so
//! `libkernel32.a` is never asked and the DLL stops importing them. The dead function bodies
//! stay in the DLL, bound to the functions below instead of to kernel32.
//!
//! Two consequences to keep in mind:
//!
//! - Any code linked into this DLL that calls one of the five gets these failing stand-ins,
//!   silently, the `windows` crate's bindings included. This addon must never need them
//!   (`lib.rs` states it does not suspend threads), so that is the intent, but it is not a
//!   link error. Before adding a real use of one, take it out of this file.
//! - If something references the plain thunk (`SuspendThread`) instead of the `__imp_`
//!   pointer, the import library's member comes in as well and the link fails with a duplicate
//!   definition, which is the loud way round.
//!
//! `global_asm!` rather than `#[no_mangle] static`: a `cdylib` exports every `#[no_mangle]`
//! item, and the DLL's export table has to stay at `GetAddonDef` alone.

use std::arch::global_asm;
use std::ffi::c_void;

/// Stands in for `SuspendThread`: fails the way the real one does, `(DWORD)-1`.
extern "system" fn suspend_thread_absent(_thread: *mut c_void) -> u32 {
    u32::MAX
}

/// Stands in for `ResumeThread`: fails the way the real one does, `(DWORD)-1`.
extern "system" fn resume_thread_absent(_thread: *mut c_void) -> u32 {
    u32::MAX
}

/// Stands in for `GetThreadContext`: fails the way the real one does, `FALSE`.
extern "system" fn get_thread_context_absent(_thread: *mut c_void, _context: *mut c_void) -> i32 {
    0
}

/// Stands in for `SetThreadContext`: fails the way the real one does, `FALSE`.
extern "system" fn set_thread_context_absent(_thread: *mut c_void, _context: *const c_void) -> i32 {
    0
}

/// Stands in for `SetThreadPriority`: fails the way the real one does, `FALSE`.
extern "system" fn set_thread_priority_absent(_thread: *mut c_void, _priority: i32) -> i32 {
    0
}

global_asm!(
    ".section .rdata$tyrian_absent_imports,\"dr\"",
    ".p2align 3",
    ".globl __imp_SuspendThread",
    "__imp_SuspendThread:",
    ".quad {suspend_thread}",
    ".globl __imp_ResumeThread",
    "__imp_ResumeThread:",
    ".quad {resume_thread}",
    ".globl __imp_GetThreadContext",
    "__imp_GetThreadContext:",
    ".quad {get_thread_context}",
    ".globl __imp_SetThreadContext",
    "__imp_SetThreadContext:",
    ".quad {set_thread_context}",
    ".globl __imp_SetThreadPriority",
    "__imp_SetThreadPriority:",
    ".quad {set_thread_priority}",
    ".text",
    suspend_thread = sym suspend_thread_absent,
    resume_thread = sym resume_thread_absent,
    get_thread_context = sym get_thread_context_absent,
    set_thread_context = sym set_thread_context_absent,
    set_thread_priority = sym set_thread_priority_absent,
);
