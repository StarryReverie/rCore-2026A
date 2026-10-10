//! Open-file objects for the chapter 6 API exercise.
//!
//! Each successful open creates an independent offset, while cloning the same
//! `Arc<OSInode>` (including through fork) shares that offset. The underlying
//! easy-fs inode owns file identity and hard-link metadata, not per-open offsets.
//!
//! Syscalls provide checked descriptors and translated user buffers. This layer
//! implements file operations without allocating descriptors or translating user
//! pointers. Internal helpers may be designed freely within the fixed interfaces.

// Allow unused items, imports, and parameters in the exercise skeleton.
#![allow(dead_code, unused_imports, unused_variables)]

use super::{File, Stat, StatMode};
use crate::drivers::BLOCK_DEVICE;
use crate::mm::UserBuffer;
use crate::sync::UPSafeCell;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::*;
use easy_fs::{EasyFileSystem, Inode};
use lazy_static::*;

/// inode in memory
/// A wrapper around a filesystem inode
/// to implement File trait atop
pub struct OSInode {
    readable: bool,
    writable: bool,
    inner: UPSafeCell<OSInodeInner>,
}
/// The OS inode inner in 'UPSafeCell'
pub struct OSInodeInner {
    offset: usize,
    inode: Arc<Inode>,
}

impl OSInode {
    /// create a new inode in memory
    pub fn new(readable: bool, writable: bool, inode: Arc<Inode>) -> Self {
        Self {
            readable,
            writable,
            inner: unsafe { UPSafeCell::new(OSInodeInner { offset: 0, inode }) },
        }
    }
    /// Todo: Read the remaining contents of this open file.
    ///
    /// Inputs: `self` is a readable file object at its current offset.
    /// Output: A vector containing bytes from that offset through EOF.
    /// Constraints: Advance the shared offset by exactly the bytes returned;
    /// do not reset it to zero. At EOF return an empty vector. Program loading
    /// obtains the entire ELF by calling this on a newly opened file at offset 0.
    pub fn read_all(&self) -> Vec<u8> {
        let mut inner = self.inner.exclusive_access();
        let mut buffer = [0u8; 512];
        let mut data: Vec<u8> = Vec::new();
        loop {
            let len = inner.inode.read_at(inner.offset, &mut buffer);
            if len == 0 {
                break;
            }
            // Advance the open offset only by the bytes actually read.
            inner.offset += len;
            data.extend_from_slice(&buffer[..len]);
        }
        data
    }
}

lazy_static! {
    pub static ref ROOT_INODE: Arc<Inode> = {
        let efs = EasyFileSystem::open(BLOCK_DEVICE.clone());
        Arc::new(EasyFileSystem::root_inode(&efs))
    };
}

/// List all apps in the root directory
pub fn list_apps() {
    println!("/**** APPS ****");
    for app in ROOT_INODE.ls() {
        println!("{}", app);
    }
    println!("**************/");
}

bitflags! {
    ///  The flags argument to the open() system call is constructed by ORing together zero or more of the following values:
    pub struct OpenFlags: u32 {
        /// readyonly
        const RDONLY = 0;
        /// writeonly
        const WRONLY = 1 << 0;
        /// read and write
        const RDWR = 1 << 1;
        /// create new file
        const CREATE = 1 << 9;
        /// truncate file size to 0
        const TRUNC = 1 << 10;
    }
}

impl OpenFlags {
    /// Do not check validity for simplicity
    /// Return (readable, writable)
    pub fn read_write(&self) -> (bool, bool) {
        if self.is_empty() {
            (true, false)
        } else if self.contains(Self::WRONLY) {
            (false, true)
        } else {
            (true, true)
        }
    }
}

/// Todo: Open, create, or truncate a regular file in the root directory.
///
/// Inputs: `name` is a valid single-component name of at most 27 bytes;
/// `flags` uses the supplied OpenFlags definition.
/// Output: A new `Arc<OSInode>` with offset 0, or `None` if no file can be opened.
/// Constraints: Use `flags.read_write()` for permissions. CREATE creates a
/// missing file and truncates an existing file even without TRUNC. Without
/// CREATE, a missing file returns None; an existing file is truncated only
/// with TRUNC. Every successful call has an independent offset. Do not allocate
/// a file descriptor or replace the inode when truncating an existing file.
pub fn open_file(name: &str, flags: OpenFlags) -> Option<Arc<OSInode>> {
    let (readable, writable) = flags.read_write();
    if flags.contains(OpenFlags::CREATE) {
        // CREATE opens the existing file after truncating it, even without TRUNC.
        if let Some(inode) = ROOT_INODE.find(name) {
            inode.clear();
            Some(Arc::new(OSInode::new(readable, writable, inode)))
        } else {
            ROOT_INODE
                .create(name)
                .map(|inode| Arc::new(OSInode::new(readable, writable, inode)))
        }
    } else {
        ROOT_INODE.find(name).map(|inode| {
            if flags.contains(OpenFlags::TRUNC) {
                inode.clear();
            }
            Arc::new(OSInode::new(readable, writable, inode))
        })
    }
}

/// Add a hard link to an existing file in the root directory.
pub fn link_file(old_name: &str, new_name: &str) -> Option<()> {
    ROOT_INODE.link(old_name, new_name)
}

/// Remove a hard link from the root directory.
pub fn unlink_file(name: &str) -> Option<()> {
    ROOT_INODE.unlink(name)
}

impl File for OSInode {
    /// Todo: Translate current inode metadata into the syscall Stat layout.
    ///
    /// Inputs: `self` is an open object referring to a live filesystem inode.
    /// Output: `Some(Stat)` with dev 0, the inode ID, current nlink, DIR or FILE
    /// mode according to the inode type, and a zero-filled explicit pad.
    /// Constraints: Query fresh metadata on each call; other names may have
    /// been linked or unlinked since open. Preserve the offset and let the
    /// supplied syscall copy the result to the caller's address space.
    fn stat(&self) -> Option<Stat> {
        let (ino, nlink, is_dir) = self.inner.exclusive_access().inode.stat();
        Some(Stat {
            dev: 0,
            ino: ino as u64,
            mode: if is_dir {
                StatMode::DIR
            } else {
                StatMode::FILE
            },
            nlink,
            pad: [0; 7],
        })
    }
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }
    /// Todo: Read from the current offset into a translated user buffer.
    ///
    /// Inputs: `buf` contains backing slices in user virtual-address order;
    /// the caller has checked readability and provided writable mapped memory.
    /// Output: The total bytes read, at most buf.len(), or 0 at EOF.
    /// Constraints: Fill slices in order without requiring contiguous physical
    /// memory. Advance offset by actual bytes read, leaving the unused buffer
    /// suffix intact. An empty buffer returns 0 without advancing the offset.
    fn read(&self, mut buf: UserBuffer) -> usize {
        let mut inner = self.inner.exclusive_access();
        let mut total_read = 0usize;
        for slice in buf.buffers.iter_mut() {
            let read_size = inner.inode.read_at(inner.offset, *slice);
            if read_size == 0 {
                break;
            }
            inner.offset += read_size;
            total_read += read_size;
        }
        total_read
    }
    /// Todo: Write a translated user buffer at the current offset.
    ///
    /// Inputs: `buf` contains source slices in user virtual-address order;
    /// writability is checked by the caller and file capacity is sufficient.
    /// Output: The total bytes written, equal to buf.len() under the supplied
    /// filesystem's complete-write contract.
    /// Constraints: Write slices consecutively, advancing offset by actual
    /// bytes written. An empty buffer returns 0 without changing the offset.
    /// Do not translate addresses again or acquire the same inner borrow twice.
    fn write(&self, buf: UserBuffer) -> usize {
        let mut inner = self.inner.exclusive_access();
        let mut total_written = 0usize;
        for slice in buf.buffers.iter() {
            let write_size = inner.inode.write_at(inner.offset, *slice);
            // The filesystem guarantees a complete write for each slice.
            assert_eq!(write_size, slice.len());
            inner.offset += write_size;
            total_written += write_size;
        }
        total_written
    }
}
