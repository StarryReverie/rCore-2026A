//! Condition variable for the chapter 8 API exercise.
//!
//! Keep the provided types, signatures, and constructor. The two TODOs implement
//! FIFO notification and waiting with mutex reacquisition. The existing single
//! core kernel is not preempted in these operations; release internal borrows
//! before switching tasks. Notifications are not stored when no task is waiting.

// Allow unused items, imports, and parameters in the exercise skeleton.
#![allow(dead_code, unused_imports, unused_variables)]

use crate::sync::{Mutex, UPSafeCell};
use crate::task::{block_current_and_run_next, current_task, wakeup_task, TaskControlBlock};
use alloc::{collections::VecDeque, sync::Arc};

/// Condition variable structure
pub struct Condvar {
    /// Condition variable inner
    pub inner: UPSafeCell<CondvarInner>,
}

/// FIFO of tasks waiting for a notification.
pub struct CondvarInner {
    /// Tasks to make ready one at a time with signal().
    pub wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl Condvar {
    /// Create a new condition variable
    pub fn new() -> Self {
        trace!("kernel: Condvar::new");
        Self {
            inner: unsafe {
                UPSafeCell::new(CondvarInner {
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }

    /// Remove and wake the first waiter, if any.
    /// An empty queue is a no-op; do not save a notification or transfer mutex
    /// ownership. The awakened task must acquire its mutex itself.
    pub fn signal(&self) {
        let mut inner = self.inner.exclusive_access();
        if let Some(task) = inner.wait_queue.pop_front() {
            wakeup_task(task);
        }
    }

    /// Wait for a notification, returning with the supplied mutex held again.
    ///
    /// The caller holds mutex on entry. Unlock it, enqueue the current task
    /// once, release the queue borrow, and block_current_and_run_next(). After
    /// waking, call mutex.lock() before returning; it may block again. The caller
    /// remains responsible for checking its condition in a loop.
    pub fn wait(&self, mutex: Arc<dyn Mutex>) {
        trace!("kernel: Condvar::wait_with_mutex");
        mutex.unlock();
        let mut inner = self.inner.exclusive_access();
        inner.wait_queue.push_back(current_task().unwrap());
        drop(inner);
        block_current_and_run_next();
        mutex.lock();
    }
}
