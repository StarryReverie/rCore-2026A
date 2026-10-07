//! System-call dispatch for the chapter 2 API exercise.
//!
//! The single entry point to all system calls, [`syscall()`], is called
//! whenever userspace wishes to perform a system call using the `ecall`
//! instruction. In this case, the processor raises an 'Environment call from
//! U-mode' exception, which is handled as one of the cases in
//! [`crate::trap::trap_handler`].
//!
//! Complete only syscall() here. The write and exit implementations in the
//! submodules, syscall numbers, and public signature are provided. Dispatch
//! forwards arguments and results; trap handling owns the saved user registers
//! and the advancement of sepc. Internal helpers may be added in the two
//! exercise files without changing the provided interfaces.

// The exercise skeleton leaves the dispatch imports and implementations unused.
#![allow(dead_code, unused_imports, unused_variables)]

/// write syscall
const SYSCALL_WRITE: usize = 64;
/// exit syscall
const SYSCALL_EXIT: usize = 93;

mod fs;
mod process;

use fs::*;
use process::*;
/// Dispatch one user system call using the provided implementations.
///
/// syscall_id comes from a7; args contains a0, a1, and a2, in that order.
/// For SYSCALL_WRITE (64), forward the fd, buffer address cast to *const u8,
/// and byte length to sys_write, returning its isize result unchanged. Its
/// existing valid-fd, accessible-buffer, and UTF-8 assumptions still apply.
/// For SYSCALL_EXIT (93), cast args[0] to i32 and call sys_exit; args[1..] are
/// ignored and control never returns because the next application is started.
/// An unsupported ID must panic with the ID, rather than report success or add
/// a new syscall. Do not modify the trap context or advance sepc in this layer.
pub fn syscall(syscall_id: usize, args: [usize; 3]) -> isize {
    match syscall_id {
        SYSCALL_WRITE => sys_write(args[0], args[1] as *const u8, args[2]),
        SYSCALL_EXIT => sys_exit(args[0] as i32),
        _ => panic!("Unsupported syscall_id: {}", syscall_id),
    }
}
