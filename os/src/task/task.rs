//! Process control blocks and lifecycle interfaces for chapter 5.
//!
//! Parents own their children through `Arc`; children refer to parents through
//! `Weak`. Constructors return ready processes without adding them to the queue.
//! New processes start with stride 0 and priority 16. Internal helpers may be
//! designed freely while preserving the supplied data structures and interfaces.

// Allow unused items, imports, and parameters in the exercise skeleton.
#![allow(dead_code, unused_imports, unused_variables)]

use super::TaskContext;
use super::{kstack_alloc, pid_alloc, KernelStack, PidHandle};
use crate::config::TRAP_CONTEXT_BASE;
use crate::mm::{MemorySet, PhysPageNum, VirtAddr, KERNEL_SPACE};
use crate::sync::UPSafeCell;
use crate::trap::{trap_handler, TrapContext};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::cell::RefMut;

/// Task control block structure
///
/// Directly save the contents that will not change during running
pub struct TaskControlBlock {
    // Immutable
    /// Process identifier
    pub pid: PidHandle,

    /// Kernel stack corresponding to PID
    pub kernel_stack: KernelStack,

    /// Mutable
    inner: UPSafeCell<TaskControlBlockInner>,
}

impl TaskControlBlock {
    /// Get the mutable reference of the inner TCB
    pub fn inner_exclusive_access(&self) -> RefMut<'_, TaskControlBlockInner> {
        self.inner.exclusive_access()
    }
    /// Get the address of app's page table
    pub fn get_user_token(&self) -> usize {
        let inner = self.inner_exclusive_access();
        inner.memory_set.token()
    }
}

pub struct TaskControlBlockInner {
    /// The physical page number of the frame where the trap context is placed
    pub trap_cx_ppn: PhysPageNum,

    /// Application data can only appear in areas
    /// where the application address space is lower than base_size
    pub base_size: usize,

    /// Save task context
    pub task_cx: TaskContext,

    /// Maintain the execution status of the current process
    pub task_status: TaskStatus,

    /// Accumulated scheduling stride.
    pub stride: usize,

    /// Scheduling priority, at least 2.
    pub prio: usize,

    /// Application address space
    pub memory_set: MemorySet,

    /// Parent process of the current process.
    /// Weak will not affect the reference count of the parent
    pub parent: Option<Weak<TaskControlBlock>>,

    /// A vector containing TCBs of all child processes of the current process
    pub children: Vec<Arc<TaskControlBlock>>,

    /// It is set when active exit or execution error occurs
    pub exit_code: i32,

    /// Heap bottom
    pub heap_bottom: usize,

    /// Program break
    pub program_brk: usize,
}

impl TaskControlBlockInner {
    /// get the trap context
    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        self.trap_cx_ppn.get_mut()
    }
    /// get the user token
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    fn get_status(&self) -> TaskStatus {
        self.task_status
    }
    pub fn is_zombie(&self) -> bool {
        self.get_status() == TaskStatus::Zombie
    }
}

impl TaskControlBlock {
    /// Todo: Create a process with an independent address space from ELF.
    ///
    /// Inputs: `elf_data` is a valid application ELF image from the loader.
    /// Output: A `Ready` process with its own PID, kernel stack, user memory,
    /// and contexts suitable for its first entry into the application.
    /// Constraints: Initialize the complete trap context and a task context for
    /// `trap_return`. Set base_size, heap_bottom, and program_brk to the initial
    /// user stack top. Start with no parent or children, exit code 0, stride 0,
    /// and priority 16. Do not enqueue the process.
    pub fn new(elf_data: &[u8]) -> Self {
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        let pid_handle = pid_alloc();
        let kernel_stack = kstack_alloc();
        let kernel_stack_top = kernel_stack.get_top();
        let task_control_block = Self {
            pid: pid_handle,
            kernel_stack,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    trap_cx_ppn,
                    base_size: user_sp,
                    task_cx: TaskContext::goto_trap_return(kernel_stack_top),
                    task_status: TaskStatus::Ready,
                    stride: 0,
                    prio: 16,
                    memory_set,
                    parent: None,
                    children: Vec::new(),
                    exit_code: 0,
                    heap_bottom: user_sp,
                    program_brk: user_sp,
                })
            },
        };
        let trap_cx = task_control_block.inner_exclusive_access().get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            kernel_stack_top,
            trap_handler as usize,
        );
        task_control_block
    }

    /// Todo: Create a child that starts executing the supplied ELF.
    ///
    /// Inputs: `self` is the parent; `elf_data` is a valid application ELF image.
    /// Output: An `Arc` to a new `Ready` child, also owned by the parent's
    /// children list, with a weak reference back to the parent.
    /// Constraints: Reuse `new` for the child's complete execution environment
    /// and default scheduling attributes. The caller enqueues the child.
    pub fn spawn(self: &Arc<Self>, elf_data: &[u8]) -> Arc<Self> {
        let child = Arc::new(Self::new(elf_data));
        child.inner_exclusive_access().parent = Some(Arc::downgrade(self));
        self.inner_exclusive_access()
            .children
            .push(Arc::clone(&child));
        child
    }

    /// Todo: Replace the current process's application with a new ELF image.
    ///
    /// Inputs: `self` is the running process; `elf_data` is a valid ELF image.
    /// Output: `()`; the next trap return enters the new application.
    /// Constraints: Replace the user address space and trap-context page,
    /// initialize the complete trap context, and reset base_size, heap_bottom,
    /// and program_brk to the new user stack top. Preserve PID, kernel stack,
    /// task context, status, family relationships, exit code, stride, and priority.
    /// Release the old user address space without creating or enqueueing a task.
    pub fn exec(&self, elf_data: &[u8]) {
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        let mut inner = self.inner_exclusive_access();
        inner.memory_set = memory_set;
        inner.trap_cx_ppn = trap_cx_ppn;
        inner.base_size = user_sp;
        inner.heap_bottom = user_sp;
        inner.program_brk = user_sp;
        let trap_cx = inner.get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            self.kernel_stack.get_top(),
            trap_handler as usize,
        );
    }

    /// Todo: Create a child with a copy of the parent's user execution state.
    ///
    /// Inputs: `self` is the running parent process.
    /// Output: An `Arc` to a new `Ready` child with its own PID, kernel stack,
    /// and independently copied user address space, including the trap context.
    /// Constraints: Inherit base_size and heap metadata, register both sides of
    /// the parent-child relationship, and initialize an empty children list,
    /// exit code 0, stride 0, and priority 16. The child's task context enters
    /// `trap_return`; its trap context uses the child's kernel stack. Preserve
    /// the parent's execution state. `sys_fork` sets child a0 to 0 and enqueues it.
    pub fn fork(self: &Arc<Self>) -> Arc<Self> {
        let mut parent_inner = self.inner_exclusive_access();
        let memory_set = MemorySet::from_existed_user(&parent_inner.memory_set);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        let pid_handle = pid_alloc();
        let kernel_stack = kstack_alloc();
        let kernel_stack_top = kernel_stack.get_top();
        let task_control_block = Arc::new(TaskControlBlock {
            pid: pid_handle,
            kernel_stack,
            inner: unsafe {
                UPSafeCell::new(TaskControlBlockInner {
                    trap_cx_ppn,
                    base_size: parent_inner.base_size,
                    task_cx: TaskContext::goto_trap_return(kernel_stack_top),
                    task_status: TaskStatus::Ready,
                    stride: 0,
                    prio: 16,
                    memory_set,
                    parent: Some(Arc::downgrade(self)),
                    children: Vec::new(),
                    exit_code: 0,
                    heap_bottom: parent_inner.heap_bottom,
                    program_brk: parent_inner.program_brk,
                })
            },
        });
        parent_inner.children.push(task_control_block.clone());
        let trap_cx = task_control_block.inner_exclusive_access().get_trap_cx();
        trap_cx.kernel_sp = kernel_stack_top;
        task_control_block
    }

    /// Todo: Query and reap one matching zombie child without blocking.
    ///
    /// Inputs: `pid == -1` matches any child; otherwise match the child's PID.
    /// Output: `Ok((pid, exit_code))` after reaping a zombie, `Err(-1)` if no
    /// child matches, or `Err(-2)` if children match but none is a zombie.
    /// Constraints: Reap the first matching zombie in children-list order.
    /// Successful reaping removes the child from the parent's
    /// list and releases its remaining resources. A child is reaped only once;
    /// errors leave the family unchanged. Do not schedule or access user pointers;
    /// the supplied syscall writes the exit code back to user space.
    pub fn waitpid(&self, pid: isize) -> Result<(usize, i32), isize> {
        let mut inner = self.inner_exclusive_access();
        if !inner
            .children
            .iter()
            .any(|p| pid == -1 || pid as usize == p.getpid())
        {
            return Err(-1);
        }
        let pair = inner.children.iter().enumerate().find(|(_, p)| {
            p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        });
        if let Some((idx, _)) = pair {
            let child = inner.children.remove(idx);
            let found_pid = child.getpid();
            let exit_code = child.inner_exclusive_access().exit_code;
            Ok((found_pid, exit_code))
        } else {
            Err(-2)
        }
    }

    /// Todo: Update this process's scheduling priority.
    ///
    /// Inputs: `prio` is the requested priority as a signed integer.
    /// Output: Return `prio` on success, or -1 when `prio < 2`.
    /// Constraints: Accept every `prio >= 2`, storing it as `usize`; an invalid
    /// request leaves the old priority unchanged. Do not reset stride or yield.
    pub fn set_priority(&self, prio: isize) -> isize {
        if prio < 2 {
            return -1;
        }
        self.inner_exclusive_access().prio = prio as usize;
        prio
    }

    /// get pid of process
    pub fn getpid(&self) -> usize {
        self.pid.0
    }

    /// change the location of the program break. return None if failed.
    pub fn change_program_brk(&self, size: i32) -> Option<usize> {
        let mut inner = self.inner_exclusive_access();
        let heap_bottom = inner.heap_bottom;
        let old_break = inner.program_brk;
        let new_brk = inner.program_brk as isize + size as isize;
        if new_brk < heap_bottom as isize {
            return None;
        }
        let result = if size < 0 {
            inner
                .memory_set
                .shrink_to(VirtAddr(heap_bottom), VirtAddr(new_brk as usize))
        } else {
            inner
                .memory_set
                .append_to(VirtAddr(heap_bottom), VirtAddr(new_brk as usize))
        };
        if result {
            inner.program_brk = new_brk as usize;
            Some(old_break)
        } else {
            None
        }
    }
}

#[derive(Copy, Clone, PartialEq)]
/// Process execution status; zombies remain until their parent reaps them.
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// Exited and awaiting reaping by the parent
    Zombie,
}
