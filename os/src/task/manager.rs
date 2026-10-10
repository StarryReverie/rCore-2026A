//! Ready queue and stride scheduling for chapter 5.
//!
//! Only `Ready` processes belong in this queue. Queue insertion is supplied;
//! selection and stride accounting are the exercise interface.

// Allow unused items, imports, and parameters in the exercise skeleton.
#![allow(dead_code, unused_imports, unused_variables)]

use super::TaskControlBlock;
use crate::config::BIG_STRIDE;
use crate::sync::UPSafeCell;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use lazy_static::*;
/// Ready processes in insertion order, accessed through the global UPSafeCell.
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

/// A stride scheduler over the ready queue.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    /// Todo: Select and remove the ready process with the smallest stride.
    ///
    /// Inputs: The ready queue; all queued processes have priority at least 2.
    /// Output: `Some(task)` for the selected process, or `None` for an empty queue.
    /// Constraints: Break equal-stride ties by queue order. Increase only the
    /// selected process's stride by `BIG_STRIDE / prio`, using `usize` integer
    /// arithmetic and the supplied `BIG_STRIDE = 1 << 16`. Preserve the order
    /// of the remaining processes. Leave the selected process `Ready`;
    /// `run_tasks` owns the transition to `Running` and the context switch.
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        let index = self
            .ready_queue
            .iter()
            .enumerate()
            .min_by_key(|(_, task)| task.inner_exclusive_access().stride)?
            .0;
        let task = self.ready_queue.remove(index)?;
        {
            let mut inner = task.inner_exclusive_access();
            inner.stride += BIG_STRIDE / inner.prio;
        }
        Some(task)
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}
