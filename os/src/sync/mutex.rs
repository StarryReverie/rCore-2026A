//! Mutexes for the chapter 8 API exercise.
//!
//! Keep the provided types, signatures, constructors, and resource accessors.
//! The four TODOs implement yielding and blocking mutexes, including resource
//! accounting. Internal helpers may be added within the exercise files.
//! Release all internal borrows before switching tasks. Blocking unlock hands
//! ownership directly to the first waiter before making it ready.

// Allow unused items, imports, and parameters in the exercise skeleton.
#![allow(dead_code, unused_imports, unused_variables)]

use super::{Resource, UPSafeCell};
use crate::task::TaskControlBlock;
use crate::task::{block_current_and_run_next, suspend_current_and_run_next};
use crate::task::{current_task, wakeup_task};
use alloc::{collections::VecDeque, sync::Arc};

/// Mutex trait
pub trait Mutex: Sync + Send {
    /// Resource accounting for this mutex.
    fn resource(&self) -> &Resource;
    /// Lock the mutex
    fn lock(&self);
    /// Unlock the mutex
    fn unlock(&self);
}

/// Spinlock Mutex struct
pub struct MutexSpin {
    locked: UPSafeCell<bool>,
    resource: Resource,
}

impl MutexSpin {
    /// Create a new spinlock mutex
    pub fn new(resource: Resource) -> Self {
        Self {
            locked: unsafe { UPSafeCell::new(false) },
            resource,
        }
    }
}

impl Mutex for MutexSpin {
    fn resource(&self) -> &Resource {
        &self.resource
    }

    /// Acquire the mutex, yielding while it is held.
    ///
    /// Record the current request with Resource::wait even when detection is
    /// disabled. On success set locked and account for the current task with
    /// Resource::acquire exactly once. On contention release the locked borrow,
    /// suspend_current_and_run_next(), and retry; do not block or busy-wait.
    /// The syscall performs rejection; this method also serves Condvar::wait.
    fn lock(&self) {
        trace!("kernel: MutexSpin::lock");
        self.resource.wait();
        loop {
            let mut locked = self.locked.exclusive_access();
            if *locked {
                drop(locked);
                suspend_current_and_run_next();
                continue;
            }
            *locked = true;
            drop(locked);
            self.resource.acquire(&current_task().unwrap());
            return;
        }
    }

    /// Release one accounted unit from the current task and clear locked.
    /// The caller holds the mutex. There is no blocking wait queue to wake.
    fn unlock(&self) {
        trace!("kernel: MutexSpin::unlock");
        self.resource.release();
        *self.locked.exclusive_access() = false;
    }
}

/// Blocking Mutex struct
pub struct MutexBlocking {
    inner: UPSafeCell<MutexBlockingInner>,
    resource: Resource,
}

/// Lock state and FIFO of blocked tasks awaiting ownership.
pub struct MutexBlockingInner {
    locked: bool,
    wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl MutexBlocking {
    /// Create a new blocking mutex
    pub fn new(resource: Resource) -> Self {
        trace!("kernel: MutexBlocking::new");
        Self {
            resource,
            inner: unsafe {
                UPSafeCell::new(MutexBlockingInner {
                    locked: false,
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }
}

impl Mutex for MutexBlocking {
    fn resource(&self) -> &Resource {
        &self.resource
    }

    /// Acquire the mutex or enqueue the current task and block.
    ///
    /// Record Resource::wait first. If free, set locked and acquire the resource
    /// for the current task. Otherwise enqueue it once at the back, release the
    /// inner borrow, and block_current_and_run_next(). On resumption unlock has
    /// already assigned ownership; do not acquire or enqueue a second time.
    fn lock(&self) {
        trace!("kernel: MutexBlocking::lock");
        self.resource.wait();
        let mut mutex_inner = self.inner.exclusive_access();
        if mutex_inner.locked {
            mutex_inner.wait_queue.push_back(current_task().unwrap());
            drop(mutex_inner);
            block_current_and_run_next();
        } else {
            mutex_inner.locked = true;
            drop(mutex_inner);
            self.resource.acquire(&current_task().unwrap());
        }
    }

    /// Release the held mutex, handing it to the first waiter if present.
    ///
    /// Require locked, then release the current task's resource accounting.
    /// With a waiter, assign its resource before wakeup_task and keep locked
    /// true. Only an empty queue permits clearing locked. Wake at most one task.
    fn unlock(&self) {
        trace!("kernel: MutexBlocking::unlock");
        let mut mutex_inner = self.inner.exclusive_access();
        assert!(mutex_inner.locked);
        self.resource.release();
        if let Some(waking_task) = mutex_inner.wait_queue.pop_front() {
            self.resource.acquire(&waking_task);
            wakeup_task(waking_task);
        } else {
            mutex_inner.locked = false;
        }
    }
}
