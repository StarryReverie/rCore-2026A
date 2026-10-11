//! Semaphore for the chapter 8 API exercise.
//!
//! Keep the provided types, signatures, and constructor. The two TODOs implement
//! permit acquisition and release, including resource accounting and FIFO
//! blocking. A negative count represents waiting tasks, not negative available
//! resources in the detector. Release internal borrows before switching tasks.

// Allow unused items, imports, and parameters in the exercise skeleton.
#![allow(dead_code, unused_imports, unused_variables)]

use crate::sync::{Resource, UPSafeCell};
use crate::task::{block_current_and_run_next, current_task, wakeup_task, TaskControlBlock};
use alloc::{collections::VecDeque, sync::Arc};

/// semaphore structure
pub struct Semaphore {
    /// semaphore inner
    pub inner: UPSafeCell<SemaphoreInner>,
    resource: Resource,
}

/// Signed permit count and FIFO of blocked tasks.
pub struct SemaphoreInner {
    /// Available permits, or minus the number of waiting tasks when negative.
    pub count: isize,
    /// Tasks awaiting a permit handed over by up().
    pub wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl Semaphore {
    /// Create a new semaphore
    pub fn new(res_count: usize, resource: Resource) -> Self {
        trace!("kernel: Semaphore::new");
        Self {
            resource,
            inner: unsafe {
                UPSafeCell::new(SemaphoreInner {
                    count: res_count as isize,
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }

    /// Return one permit and wake at most the first waiting task.
    ///
    /// Release resource accounting and increment count once. If the new count
    /// is nonpositive, pop the first waiter, assign it one resource unit, then
    /// wake it. Do not increment count again for the handoff. The provided
    /// Resource::release also supports event notification by a task that did
    /// not previously call down(); do not add an ownership requirement.
    pub fn up(&self) {
        trace!("kernel: Semaphore::up");
        let mut inner = self.inner.exclusive_access();
        self.resource.release();
        inner.count += 1;
        if inner.count <= 0 {
            if let Some(task) = inner.wait_queue.pop_front() {
                self.resource.acquire(&task);
                wakeup_task(task);
            }
        }
    }

    /// Acquire one permit, returning false only if Resource::request rejects it.
    ///
    /// Check the request before changing count or the queue. On acceptance,
    /// decrement count once. A nonnegative result acquires a resource unit for
    /// the current task immediately; a negative result enqueues it once, drops
    /// the inner borrow, and blocks. On resumption up() has already assigned
    /// the permit, so do not decrement or acquire again. Return true on success;
    /// the syscall wrapper translates rejection into -0xDEAD.
    pub fn down(&self) -> bool {
        trace!("kernel: Semaphore::down");
        if !self.resource.request() {
            return false;
        }
        let mut inner = self.inner.exclusive_access();
        inner.count -= 1;
        if inner.count < 0 {
            inner.wait_queue.push_back(current_task().unwrap());
            drop(inner);
            block_current_and_run_next();
        } else {
            drop(inner);
            self.resource.acquire(&current_task().unwrap());
        }
        true
    }
}
