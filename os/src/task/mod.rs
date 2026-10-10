//! Process lifecycle and scheduling exercise for chapter 5.
//!
//! The supplied task control blocks, ready queue, and processor state share one
//! representation of each process. Constructors leave tasks ready but not queued;
//! callers enqueue them. A running task is owned by the processor and is absent
//! from the ready queue. Zombies never run again and await parental reaping.
//!
//! Scheduling passes through the processor's idle context on this single core.
//! Release all dynamic borrows before switching; context pointers must remain
//! valid during use. Do not retain temporary owning references on an exited
//! process's abandoned stack, since they would prevent its final reclamation.
//!
//! Internal helper functions may be designed freely within the fixed interfaces.

// Allow unused items, imports, and parameters in the exercise skeleton.
#![allow(dead_code, unused_imports, unused_variables)]

mod context;
mod id;
mod manager;
mod processor;
mod switch;
#[allow(clippy::module_inception)]
mod task;

use crate::loader::get_app_data_by_name;
use alloc::sync::Arc;
use lazy_static::*;
pub use manager::{fetch_task, TaskManager};
use switch::__switch;
pub use task::{TaskControlBlock, TaskStatus};

pub use context::TaskContext;
pub use id::{kstack_alloc, pid_alloc, KernelStack, PidHandle};
pub use manager::add_task;
pub use processor::{
    current_task, current_trap_cx, current_user_token, run_tasks, schedule, take_current_task,
    Processor,
};
/// Todo: Suspend the running process and return control to the scheduler.
///
/// Inputs: The processor owns the current `Running` process.
/// Output: The process is `Ready` and queued once, with a resumable task context;
/// this call returns `()` when the process is dispatched again.
/// Constraints: Support both yield and timer preemption. Clear the processor's
/// current slot before scheduling and release all dynamic borrows. Preserve
/// the process's resources and scheduling attributes; stride is charged by fetch.
pub fn suspend_current_and_run_next() {
    let task = take_current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    let task_cx_ptr = &mut task_inner.task_cx as *mut TaskContext;
    task_inner.task_status = TaskStatus::Ready;
    drop(task_inner);
    add_task(task);
    schedule(task_cx_ptr);
}

/// PID of initproc; this user process is distinct from the processor's idle context.
pub const IDLE_PID: usize = 0;

/// Todo: Terminate the running process and return control to the scheduler.
///
/// Inputs: `exit_code` is supplied by sys_exit or application trap handling.
/// Output: A non-init process becomes `Zombie` with the recorded exit code;
/// the scheduler resumes and the exiting process never returns to its caller.
/// Constraints: Clear `current`, transfer all children to INITPROC with updated
/// parent links, and recycle the exiting process's user data pages. Its parent
/// retains it until waitpid reaps the TCB, PID, and kernel stack. Release borrows
/// and temporary owning references before switching; never enqueue a zombie.
/// Exiting PID 0 terminates the kernel with the existing panic policy.
pub fn exit_current_and_run_next(exit_code: i32) {
    let task = take_current_task().unwrap();
    let pid = task.getpid();
    if pid == IDLE_PID {
        println!(
            "[kernel] Idle process exit with exit_code {} ...",
            exit_code
        );
        panic!("All applications completed!");
    }
    let mut inner = task.inner_exclusive_access();
    inner.task_status = TaskStatus::Zombie;
    inner.exit_code = exit_code;
    {
        let mut initproc_inner = INITPROC.inner_exclusive_access();
        for child in inner.children.iter() {
            child.inner_exclusive_access().parent = Some(Arc::downgrade(&INITPROC));
            initproc_inner.children.push(child.clone());
        }
    }
    inner.children.clear();
    inner.memory_set.recycle_data_pages();
    drop(inner);
    drop(task);
    let mut _unused = TaskContext::zero_init();
    schedule(&mut _unused as *mut _);
}

lazy_static! {
    /// Creation of initial process
    ///
    /// the name "initproc" may be changed to any other app name like "usertests",
    /// but we have user_shell, so we don't need to change it.
    pub static ref INITPROC: Arc<TaskControlBlock> = Arc::new(TaskControlBlock::new(
        get_app_data_by_name("ch5b_initproc").unwrap()
    ));
}

///Add init process to the manager
pub fn add_initproc() {
    add_task(INITPROC.clone());
}
