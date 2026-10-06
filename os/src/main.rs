//! Bare-metal Rust entry point for the chapter 1 API exercise.
//!
//! The provided assembly entry sets the stack pointer before calling
//! [`rust_main()`]. Complete this one function while preserving the entry
//! attributes, linker-symbol declarations, and all supporting implementations.
//!
//! No host runtime initializes this program. [`clear_bss()`] establishes zeroed
//! global storage; console output uses the provided SBI wrapper. Successful
//! execution ends through the board's QEMU exit device rather than returning.

#![deny(missing_docs)]
#![deny(warnings)]
#![no_std]
#![no_main]
#![feature(panic_info_message)]

use core::arch::global_asm;
// These macros are used when completing rust_main().
#[allow(unused_imports)]
use log::*;

#[macro_use]
mod console;
mod lang_items;
// The provided logger and success-exit path are unused until the TODO is filled.
#[allow(dead_code)]
mod logging;
mod sbi;

#[path = "boards/qemu.rs"]
#[allow(dead_code)]
mod board;

global_asm!(include_str!("entry.asm"));

/// Clear [sbss, ebss), excluding the active boot stack placed before sbss.
/// This implementation is provided; call it before initializing global state.
pub fn clear_bss() {
    extern "C" {
        fn sbss();
        fn ebss();
    }
    (sbss as usize..ebss as usize).for_each(|a| unsafe { (a as *mut u8).write_volatile(0) });
}

/// Initialize the bare-metal program, run a calculation, print its layout, and
/// terminate QEMU successfully. This is the only exercise interface.
///
/// The assembly entry has already established a 64 KiB stack. Call clear_bss()
/// before logging::init(), which must run once. Print `[kernel] Hello, world!`,
/// then sum the local usize array [1, 2, 3, 4, 5] by traversing its elements and
/// print `[kernel] sum = 15` using the computed result, not a hard-coded sum.
///
/// Preserve the existing log levels: trace! for .text, debug! for .rodata,
/// info! for .data, warn! for the boot-stack bounds, and error! for .bss, in that
/// order. Obtain hexadecimal addresses from the linker symbols below; they are
/// addresses, not callable functions. Keep the logger's LOG filtering, including
/// disabled logs when LOG is unset. Do not clear the boot stack.
///
/// Finish with QEMU_EXIT_HANDLE.exit_success(). The return type `!` forbids
/// returning to the assembly caller; panic, sbi::shutdown() (failure exit), or
/// an endless loop are not successful completion. No std or heap is available.
#[no_mangle]
#[allow(dead_code, unused_imports)]
pub fn rust_main() -> ! {
    use crate::board::QEMUExit;

    extern "C" {
        fn stext(); // begin addr of text segment
        fn etext(); // end addr of text segment
        fn srodata(); // start addr of Read-Only data segment
        fn erodata(); // end addr of Read-Only data segment
        fn sdata(); // start addr of data segment
        fn edata(); // end addr of data segment
        fn sbss(); // start addr of BSS segment
        fn ebss(); // end addr of BSS segment
        fn boot_stack_lower_bound(); // stack lower bound
        fn boot_stack_top(); // stack top
    }

    // 1. 未初始化的全局存储先清零，之后才能安全地建立日志等全局状态。
    clear_bss();
    logging::init();

    // 2. 输出启动信息（由提供的 SBI 控制台，不依赖宿主标准输出）。
    println!("[kernel] Hello, world!");

    // 3. 在启动栈上定义局部数组，实际遍历元素求和并输出计算结果。
    let numbers: [usize; 5] = [1, 2, 3, 4, 5];
    let mut sum: usize = 0;
    for number in numbers.iter() {
        sum += *number;
    }
    println!("[kernel] sum = {}", sum);

    // 4. 按原有日志宏、级别、格式与顺序记录内存布局，地址取自链接符号。
    trace!(
        "[kernel] .text [{:#x}, {:#x})",
        stext as usize,
        etext as usize
    );
    debug!(
        "[kernel] .rodata [{:#x}, {:#x})",
        srodata as usize, erodata as usize
    );
    info!(
        "[kernel] .data [{:#x}, {:#x})",
        sdata as usize, edata as usize
    );
    warn!(
        "[kernel] boot_stack top=bottom={:#x}, lower_bound={:#x}",
        boot_stack_top as usize, boot_stack_lower_bound as usize
    );
    error!("[kernel] .bss [{:#x}, {:#x})", sbss as usize, ebss as usize);

    // 5. 通过已提供的 QEMU 退出设备以成功状态结束。
    crate::board::QEMU_EXIT_HANDLE.exit_success();
}
