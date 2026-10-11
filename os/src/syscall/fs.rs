//! File and filesystem-related syscalls
//!
//! The chapter 7 API exercise replaces only sys_pipe and sys_dup. Other syscalls
//! are provided, including descriptor/permission checks and user-buffer
//! translation for read/write. Preserve their behavior and all public signatures.

// Allow imports and parameters used by the two exercise interfaces.
#![allow(unused_imports, unused_variables)]

use crate::config::PAGE_SIZE;
use crate::fs::{link_file, make_pipe, open_file, unlink_file, OpenFlags, Stat};
use crate::mm::{
    translated_byte_buffer, translated_refmut, translated_str, MapPermission, PageTable,
    UserBuffer, VirtAddr,
};
use crate::task::{current_task, current_user_token};
use alloc::sync::Arc;
use core::mem::size_of;

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_write", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        if !file.writable() {
            return -1;
        }
        let file = file.clone();
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_read", current_task().unwrap().pid.0);
    let token = current_user_token();
    let task = current_task().unwrap();
    let inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        if !file.readable() {
            return -1;
        }
        // release current task TCB manually to avoid multi-borrow
        drop(inner);
        trace!("kernel: sys_read .. file.read");
        file.read(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_open(path: *const u8, flags: u32) -> isize {
    trace!("kernel:pid[{}] sys_open", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(inode) = open_file(path.as_str(), OpenFlags::from_bits(flags).unwrap()) {
        let mut inner = task.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(inode);
        fd as isize
    } else {
        -1
    }
}

pub fn sys_close(fd: usize) -> isize {
    trace!("kernel:pid[{}] sys_close", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    inner.fd_table[fd].take();
    0
}

/// Create a pipe, install both endpoints, and return 0 (syscall 59).
///
/// `pipe` points to two consecutive, aligned, writable usize values in the
/// current process's address space; address arithmetic and allocations succeed.
/// Allocate the smallest free descriptor for the read endpoint and occupy it
/// before allocating the write descriptor. Preserve every existing descriptor.
/// Write back [read_fd, write_fd] using the current token and provided translation
/// helpers, not by dereferencing user virtual addresses in the kernel.
/// Invalid user pointers and allocation-failure rollback are outside the input
/// contract. No new address-translation or descriptor-management API is needed.
pub fn sys_pipe(pipe: *mut usize) -> isize {
    trace!("kernel:pid[{}] sys_pipe", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let token = current_user_token();
    let mut inner = task.inner_exclusive_access();
    let (pipe_read, pipe_write) = make_pipe();
    // Occupy the read slot before allocating the write descriptor so the two
    // descriptors are always distinct.
    let read_fd = inner.alloc_fd();
    inner.fd_table[read_fd] = Some(pipe_read);
    let write_fd = inner.alloc_fd();
    inner.fd_table[write_fd] = Some(pipe_write);
    // Translate each result slot independently; the two usize values may map to
    // non-contiguous physical pages.
    *translated_refmut(token, pipe) = read_fd;
    let write_ptr = (pipe as usize + size_of::<usize>()) as *mut usize;
    *translated_refmut(token, write_ptr) = write_fd;
    0
}

/// Duplicate a descriptor into the smallest free slot (syscall 24).
///
/// Return -1 without changing the table if fd is out of range or closed.
/// Otherwise clone the existing Arc<dyn File>, install it in the newly allocated
/// slot, and return that descriptor. Do not create or reopen a file: duplicates
/// share offsets for ordinary files and extend endpoint lifetimes for pipes.
/// The original descriptor and all other occupied slots remain unchanged.
pub fn sys_dup(fd: usize) -> isize {
    trace!("kernel:pid[{}] sys_dup", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    let file = inner.fd_table[fd].as_ref().unwrap().clone();
    let new_fd = inner.alloc_fd();
    inner.fd_table[new_fd] = Some(file);
    new_fd as isize
}

/// Get an open file's metadata and copy it into the caller's address space.
pub fn sys_fstat(fd: usize, st: *mut Stat) -> isize {
    trace!("kernel:pid[{}] sys_fstat", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let file = {
        let inner = task.inner_exclusive_access();
        match inner.fd_table.get(fd).and_then(|entry| entry.as_ref()) {
            Some(file) => file.clone(),
            None => return -1,
        }
    };
    let stat = match file.stat() {
        Some(stat) => stat,
        None => return -1,
    };
    let token = current_user_token();
    let len = size_of::<Stat>();
    let mut start = st as usize;
    let end = match start.checked_add(len) {
        Some(end) => end,
        None => return -1,
    };
    // Check the whole destination before writing, including a possible second page.
    let page_table = PageTable::from_token(token);
    while start < end {
        let va = VirtAddr::from(start);
        if usize::from(va) != start {
            return -1;
        }
        match page_table.translate(va.floor()) {
            Some(pte)
                if pte.is_valid()
                    && pte.writable()
                    && pte.flags().bits() & MapPermission::U.bits() != 0 => {}
            _ => return -1,
        }
        start += (PAGE_SIZE - va.page_offset()).min(end - start);
    }
    // Stat's C layout has no implicit padding; its explicit pad is initialized.
    let bytes = unsafe { core::slice::from_raw_parts(&stat as *const Stat as *const u8, len) };
    let mut offset = 0;
    for buffer in translated_byte_buffer(token, st as *const u8, len) {
        let next = offset + buffer.len();
        buffer.copy_from_slice(&bytes[offset..next]);
        offset = next;
    }
    0
}

/// Create a second directory entry referring to the same file inode.
pub fn sys_linkat(old_name: *const u8, new_name: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_linkat", current_task().unwrap().pid.0);
    let token = current_user_token();
    let old_name = translated_str(token, old_name);
    let new_name = translated_str(token, new_name);
    link_file(&old_name, &new_name).map_or(-1, |_| 0)
}

/// Remove a file name, reclaiming the file after its final hard link.
pub fn sys_unlinkat(name: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_unlinkat", current_task().unwrap().pid.0);
    let name = translated_str(current_user_token(), name);
    unlink_file(&name).map_or(-1, |_| 0)
}
