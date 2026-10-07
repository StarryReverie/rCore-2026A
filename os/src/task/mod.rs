//! Task management for chapter 3, with syscall accounting provided.
//!
//! This kernel runs on a single core. Internal helper functions may be designed freely.
//!
//! Scheduling selects `Ready` tasks in round-robin order by task ID, starting
//! after the current task and staying within `0..num_app`. While a task runs,
//! `current_task` identifies it and its state is `Running`.
//!
//! No borrow of the task manager's internal state may remain active across a
//! context switch. Context pointers must remain valid for as long as they are used.

// Allow unused items, imports, and parameters in the exercise skeleton.
#![allow(dead_code, unused_imports, unused_variables)]

mod context;
mod switch;
#[allow(clippy::module_inception)]
mod task;

use crate::config::{MAX_APP_NUM, MAX_SYSCALL_NUM};
use crate::loader::{get_num_app, init_app_cx};
use crate::sync::UPSafeCell;
use lazy_static::lazy_static;
use switch::__switch;

pub use context::TaskContext;
pub use task::{TaskControlBlock, TaskStatus};

/// Manages the statically loaded applications.
pub struct TaskManager {
    /// Number of loaded applications, in `1..=MAX_APP_NUM`.
    num_app: usize,
    /// Mutable task management state with runtime borrow checking.
    inner: UPSafeCell<TaskManagerInner>,
}

/// Mutable state of the task manager.
pub struct TaskManagerInner {
    /// Task control blocks indexed by task ID; `0..num_app` covers loaded applications.
    tasks: [TaskControlBlock; MAX_APP_NUM],
    /// ID of the current task; initialized to 0 before any task starts.
    current_task: usize,
}

lazy_static! {
    /// Todo: Initialize the global task manager for the loaded applications.
    ///
    /// Inputs: No arguments. The loader provides the application count and
    /// initial application contexts.
    /// Output: A `TaskManager` whose loaded tasks are `Ready`, each with a context
    /// for its first run, and whose `current_task` is 0.
    /// Constraints: The application count is in `1..=MAX_APP_NUM`, with one task
    /// per loaded application. Unused slots are `UnInit`, and all syscall counters
    /// start at zero.
    pub static ref TASK_MANAGER: TaskManager = {
        let num_app = get_num_app();
        // Start from all-`UnInit` slots, then bring only the loaded
        // applications online so unused slots keep the `UnInit` state.
        let mut tasks = [TaskControlBlock {
            task_status: TaskStatus::UnInit,
            task_cx: TaskContext::zero_init(),
            syscall_counts: [0; MAX_SYSCALL_NUM],
        }; MAX_APP_NUM];
        for (i, task) in tasks.iter_mut().enumerate().take(num_app) {
            // Task IDs `0..num_app` map to the loaded applications; `init_app_cx`
            // drops a `TrapContext` on the task's kernel stack and
            // `goto_restore` makes `__restore` the first thing `__switch` runs.
            task.task_cx = TaskContext::goto_restore(init_app_cx(i));
            task.task_status = TaskStatus::Ready;
        }
        TaskManager {
            num_app,
            inner: unsafe {
                UPSafeCell::new(TaskManagerInner {
                    tasks,
                    current_task: 0,
                })
            },
        }
    };
}

impl TaskManager {
    /// Give the CPU to application `0` for the first time.
    fn run_first_task(&self) -> ! {
        let mut inner = self.inner.exclusive_access();
        inner.current_task = 0;
        inner.tasks[0].task_status = TaskStatus::Running;
        let next_task_cx_ptr = &inner.tasks[0].task_cx as *const TaskContext;
        // Release the runtime borrow before switching stacks. `next_task_cx_ptr`
        // points into the static `TASK_MANAGER`, so it stays valid afterwards.
        drop(inner);
        // The startup context is not needed again; `__switch` overwrites it.
        let mut _unused = TaskContext::zero_init();
        unsafe {
            __switch(&mut _unused as *mut TaskContext, next_task_cx_ptr);
        }
        panic!("unreachable in run_first_task!");
    }

    /// Mark the current task `Ready` so it keeps its turn in the rotation.
    fn mark_current_suspended(&self) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].task_status = TaskStatus::Ready;
    }

    /// Mark the current task `Exited`, a terminal state it never leaves.
    fn mark_current_exited(&self) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].task_status = TaskStatus::Exited;
    }

    /// Find the next `Ready` task in round-robin order within `0..num_app`.
    ///
    /// The search starts just after the current task and wraps around, so the
    /// current task itself is only chosen after every other loaded task was
    /// inspected once. `UnInit` and `Exited` slots are never selected.
    fn find_next_task(&self) -> Option<usize> {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        (current + 1..=current + self.num_app)
            .map(|id| id % self.num_app)
            .find(|id| inner.tasks[*id].task_status == TaskStatus::Ready)
    }

    /// Switch from the current task to the next ready one, if any.
    fn run_next_task(&self) {
        if let Some(next) = self.find_next_task() {
            let mut inner = self.inner.exclusive_access();
            let current = inner.current_task;
            inner.tasks[next].task_status = TaskStatus::Running;
            inner.current_task = next;
            let current_task_cx_ptr = &mut inner.tasks[current].task_cx as *mut TaskContext;
            let next_task_cx_ptr = &inner.tasks[next].task_cx as *const TaskContext;
            // Release the runtime borrow before switching stacks; both context
            // pointers point into the static `TASK_MANAGER` and stay valid.
            drop(inner);
            unsafe {
                __switch(current_task_cx_ptr, next_task_cx_ptr);
            }
            // Control returns here when this task is scheduled again.
        } else {
            panic!("All applications completed!");
        }
    }
}

/// Record one system call made by the current task.
pub fn record_current_syscall(syscall_id: usize) {
    let mut inner = TASK_MANAGER.inner.exclusive_access();
    let current = inner.current_task;
    if let Some(count) = inner.tasks[current].syscall_counts.get_mut(syscall_id) {
        *count += 1;
    }
}

/// Return the current task's call count for the given syscall ID.
pub fn current_syscall_count(syscall_id: usize) -> usize {
    let inner = TASK_MANAGER.inner.exclusive_access();
    inner.tasks[inner.current_task]
        .syscall_counts
        .get(syscall_id)
        .copied()
        .unwrap_or(0)
}

/// Todo: Start the first task during kernel startup.
///
/// Inputs: No arguments. Uses `TASK_MANAGER` after the applications have been loaded.
/// Output: Application 0 begins execution; control does not return to the
/// startup code.
/// Constraints: Used only for initial task startup. Task 0 is `Running` and is
/// identified by `current_task`.
pub fn run_first_task() {
    TASK_MANAGER.run_first_task();
}

/// Todo: Suspend the current task and yield the CPU.
///
/// Inputs: No arguments. The current task in `TASK_MANAGER` is `Running`.
/// Output: Execution passes to the next ready task. This call returns `()` when
/// the suspended task resumes execution.
/// Constraints: The suspended task is `Ready` while awaiting execution and
/// retains the context needed to resume. This entry point must support both
/// voluntary yielding and timer preemption.
pub fn suspend_current_and_run_next() {
    TASK_MANAGER.mark_current_suspended();
    TASK_MANAGER.run_next_task();
}

/// Todo: Exit the current task and relinquish the CPU.
///
/// Inputs: No arguments. The current task in `TASK_MANAGER` is `Running`.
/// Output: The current task terminates and yields execution to the next ready
/// task; it never resumes at the call site.
/// Constraints: The exiting task is `Exited` and must never be scheduled again.
pub fn exit_current_and_run_next() {
    TASK_MANAGER.mark_current_exited();
    TASK_MANAGER.run_next_task();
}
