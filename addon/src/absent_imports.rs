//! Keeps `SuspendThread` and `SetThreadContext` out of this DLL's import table.
//!
//! No code in this addon calls either. They used to be imported anyway, because of how the
//! mingw runtime is linked in (see `.cargo/config.toml`): libstdc++ (`guard.o`, `eh_alloc.o`)
//! and libgcc (`emutls.o`) need `pthread_once`, `pthread_key_create`, `pthread_getspecific`,
//! `pthread_setspecific` and the mutex and condition calls, which pulls
//! `libpthread.a(libwinpthread_la-thread.o)` into the link. That member also defines
//! `pthread_cancel`, the only function in the whole link that references
//! `__imp_SuspendThread` and `__imp_SetThreadContext`, and Fedora builds winpthreads without
//! `-ffunction-sections`: the member is one `.text` section, so `--gc-sections` keeps
//! `pthread_cancel` as long as anything else in it is used. Nothing reaches it: its only
//! caller is `pthread_kill`, in the same member, and no object in the link references either
//! (measured with `-Wl,-Map=...,--cref` and `objdump -dr` of the member).
//!
//! An import only enters the table when the linker has to pull the import library's member
//! to resolve `__imp_<name>`. Defining those two symbols here resolves them first, so
//! `libkernel32.a` is never asked and the DLL stops importing them. The dead `pthread_cancel`
//! body stays in the DLL, bound to the two functions below instead of to kernel32.
//!
//! Two consequences to keep in mind:
//!
//! - Any code linked into this DLL that calls `SuspendThread` or `SetThreadContext` gets these
//!   failing stand-ins, silently, the `windows` crate's bindings included. This addon must
//!   never need them (`lib.rs` states it does not suspend threads), so that is the intent, but
//!   it is not a link error.
//! - If something references the plain `SuspendThread`/`SetThreadContext` thunks instead of
//!   the `__imp_` pointers, the import library's member comes in as well and the link fails
//!   with a duplicate definition, which is the loud way round.
//!
//! `global_asm!` rather than `#[no_mangle] static`: a `cdylib` exports every `#[no_mangle]`
//! item, and the DLL's export table has to stay at `GetAddonDef` alone.

use std::arch::global_asm;
use std::ffi::c_void;

/// Stands in for `SuspendThread`: fails the way the real one does, `(DWORD)-1`.
extern "system" fn suspend_thread_absent(_thread: *mut c_void) -> u32 {
    u32::MAX
}

/// Stands in for `SetThreadContext`: fails the way the real one does, `FALSE`.
extern "system" fn set_thread_context_absent(_thread: *mut c_void, _context: *const c_void) -> i32 {
    0
}

global_asm!(
    ".section .rdata$tyrian_absent_imports,\"dr\"",
    ".p2align 3",
    ".globl __imp_SuspendThread",
    "__imp_SuspendThread:",
    ".quad {suspend_thread}",
    ".globl __imp_SetThreadContext",
    "__imp_SetThreadContext:",
    ".quad {set_thread_context}",
    ".text",
    suspend_thread = sym suspend_thread_absent,
    set_thread_context = sym set_thread_context_absent,
);
