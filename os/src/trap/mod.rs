//! User trap handling for the chapter 2 API exercise.
//!
//! For rCore, we have a single trap entry point, namely `__alltraps`. At
//! initialization in [`init()`], we set the `stvec` CSR to point to it.
//!
//! All traps go through `__alltraps`, which is defined in `trap.S`. The
//! assembly language code saves the user context on the kernel stack and
//! transfers control to
//! [`trap_handler()`].
//!
//! Complete only trap_handler() here. It dispatches user ecalls, terminates
//! applications that cause the supported faults, and diagnoses unsupported
//! traps. This chapter has no timer preemption. Keep init(), TrapContext, and
//! the assembly save/restore path as provided.

// Allow imports and the context parameter to remain in the exercise skeleton.
#![allow(unused_imports, unused_variables)]

mod context;

use crate::batch::run_next_app;
use crate::syscall::syscall;
use core::arch::global_asm;
use riscv::register::{
    mtvec::TrapMode,
    scause::{self, Exception, Trap},
    stval, stvec,
};

global_asm!(include_str!("trap.S"));

/// initialize CSR `stvec` as the entry of `__alltraps`
pub fn init() {
    extern "C" {
        fn __alltraps();
    }
    unsafe {
        stvec::write(__alltraps as usize, TrapMode::Direct);
    }
}

#[no_mangle]
/// Handle a trap using its CSR cause and the user context saved by __alltraps.
///
/// Read scause and stval through the provided register interfaces. For
/// UserEnvCall, advance cx.sepc by exactly 4 before dispatch so execution resumes
/// after ecall. The syscall ID is cx.x[17] (a7); arguments are cx.x[10..=12]
/// (a0..a2). If syscall() returns, cast its isize result to usize into cx.x[10]
/// and return the same cx for __restore. Preserve sstatus and all other saved
/// registers. The exit syscall does not return to this context.
///
/// StoreFault and StorePageFault print the existing PageFault message, then
/// call run_next_app(), which does not return. IllegalInstruction prints the
/// existing IllegalInstruction message and also runs the next application.
/// Do not resume a faulting application or advance its sepc as if it made an
/// ecall. The messages are:
/// `[kernel] PageFault in application, kernel killed it.`
/// `[kernel] IllegalInstruction in application, kernel killed it.`
///
/// All other causes, including unsupported interrupts, panic with the cause
/// and stval. Do not add scheduling or fault recovery. Application loading,
/// context restoration, and successful shutdown after the last app are provided.
pub fn trap_handler(cx: &mut TrapContext) -> &mut TrapContext {
    let scause = scause::read();
    let stval = stval::read();
    match scause.cause() {
        Trap::Exception(Exception::UserEnvCall) => {
            // The saved sepc points at the ecall itself; skip it so that the
            // restore path resumes at the following instruction.
            cx.sepc += 4;
            // Read a7/a0..a2 before writing the result back to a0.
            cx.x[10] = syscall(cx.x[17], [cx.x[10], cx.x[11], cx.x[12]]) as usize;
        }
        Trap::Exception(Exception::StoreFault) | Trap::Exception(Exception::StorePageFault) => {
            println!("[kernel] PageFault in application, kernel killed it.");
            run_next_app();
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            println!("[kernel] IllegalInstruction in application, kernel killed it.");
            run_next_app();
        }
        _ => {
            panic!(
                "Unsupported trap {:?}, stval = {:#x}!",
                scause.cause(),
                stval
            );
        }
    }
    cx
}

pub use context::TrapContext;
