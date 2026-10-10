# 第 6 章 文件系统与硬链接 实验报告

## 0. 实验对象与环境

| 项 | 内容 |
| --- | --- |
| 分析对象 | `ch6` 分支参考实现（easy-fs 文件/目录/硬链接 + OS 打开对象） |
| 独立实现 | `ch6-api` 分支：`easy-fs/src/vfs.rs` 9 处 + `os/src/fs/inode.rs` 5 处 TODO |
| 构建 | `make build BASE=2`；GDB 跟踪用 `make build MODE=debug BASE=2` |
| 运行/验收 | `make run BASE=2`，在用户 shell 内运行 `ch6b_usertest`、`ch6_usertest` |
| 调试器 | NixOS 无 `riscv64-unknown-elf-gdb`；用多架构 `gdb` 17.2 + `make gdbserver MODE=debug BASE=2 GDB=gdb` |
| 日志 | `reports/ch6-gdb.log`、`reports/ch6-api-impl-run.log`；脚本 `reports/ch6-gdb.cmd` |

GDB 会话固定设置：

```gdb
set pagination off
set confirm off
set architecture riscv:rv64
set language c
target remote localhost:1234
```

---

## 1. 静态分析

### 1.1 四层对象与分工

| 层 | 位置 | 保存的状态 | 生命周期 |
| --- | --- | --- | --- |
| 磁盘 inode `DiskInode` | 磁盘 inode 区 | `size`、`nlink`、`direct[27]`、`indirect1/2`、`type_` | 与文件同生共死 |
| 目录项 `DirEntry` | 目录 inode 的数据块 | 27 字节名字 + `inode_id`（`DIRENT_SZ = 32`） | 名称存在即存在 |
| VFS 句柄 `easy_fs::Inode` | 内存 | `block_id`、`block_offset`、`Arc<Mutex<EasyFileSystem>>`、块设备 | 一次查找/创建产生一个 |
| 打开对象 `OSInode` | 内存 | `readable`、`writable`、`inner: UPSafeCell<{offset, Arc<Inode>}>` | 一次 `open` 产生一个 |

关键点：**文件身份在磁盘 inode，读写偏移在打开对象**。`Inode` 不保存偏移，所以同一文件的两次 `open` 各有一个 `offset`；而 `fork` 复制文件描述符表里的 `Arc<dyn File>`，父子共享同一个 `OSInode`，因此共享偏移（见 1.2）。

### 1.2 打开与偏移

`open_file(name, flags)` 的语义（与 API 契约一致）：

| 条件 | 行为 |
| --- | --- |
| 有 `CREATE`，不存在 | `create` 新文件后打开 |
| 有 `CREATE`，已存在 | `clear()` 截断后打开（**即使没有 `TRUNC`**），不替换 inode |
| 无 `CREATE`，存在且有 `TRUNC` | `clear()` 后打开 |
| 无 `CREATE`，存在且无 `TRUNC` | 保留内容打开 |
| 无 `CREATE`，不存在 | 返回 `None` |

`OSInode::new` 把 `offset` 置 0；`File::read/write` 每次按实际读写字节推进该偏移；`read_all` 也从当前偏移读到 EOF。`exec`/`spawn`/初始进程用新打开（偏移 0）的对象加载完整 ELF。

### 1.3 目录项、创建与硬链接

- `find`：`find_inode_id` 遍历目录数据块里的 `DirEntry`，**跳过 `inode_id == 0` 的空槽**，命中后按 inode 编号算出磁盘位置并构造新 `Inode`。
- `create`：先查重（同名返回 `None`，不改状态），再 `alloc_inode` → `DiskInode::initialize(File)`（`size=0, nlink=1`）→ `append_dirent` 写一条目录项。`append_dirent` 会**复用第一个空槽**，没有空槽才 `increase_size` 扩展目录。
- `link`：`old_name` 的目标必须是普通文件；插入一条引用同一 inode 的新目录项，并把 `nlink` 加一。名称校验拒绝空、NUL、`/`、`.`、`..`，并用 `checked_add` 防溢出。
- `unlink`：把目录项写成 `DirEntry::empty()`（不缩短目录），`nlink` 用 `checked_sub` 减一。**只有减到 0** 才回收数据块并在位图中归还 inode；仍有链接时数据完好。

### 1.4 截断与资源回收

- `clear`：只调用 `clear_inode_data`，即 `DiskInode::clear_size` 返回「直接 + 一级 + 二级间接」全部数据块，再 `dealloc_data` 清零并归还位图；保留 inode 编号、类型、`nlink` 和所有目录项。对一个硬链接截断后，其他名字看到同一个空文件。
- `unlink` 走到 `nlink == 0` 时复用 `clear_inode_data` 回收数据块，再 `dealloc_inode` 归还 inode 编号。**资源归属由磁盘 `nlink` 决定，不是内存 `Arc` 的强引用数**：`Arc` 只是句柄计数，删除最后一个名字即回收，不延迟。

### 1.5 锁与块缓存约束

- `EasyFileSystem` 包在不可重入的 `spin::Mutex` 里；持有它时不能再调用会再次 `self.fs.lock()` 的公开方法（否则死锁）。因此 `create/link/unlink` 都在**一次取锁**内用 `find_inode_id`/`find_dirent` 完成查找与修改。
- 块缓存共 16 槽，`get_block_cache` 返回的 `Mutex<BlockCache>` 也不可重入；**目录 inode 与目标 inode 可能落在同一个磁盘块**。所以「读目标 `nlink` → 改目录项 → 写回 `nlink`」必须拆成独立的加解锁语句，不能同时持有两次同一块的守卫。
- 所有公开操作在返回前 `block_cache_sync_all()`，且同步前不持有缓存守卫。

---

## 2. GDB 动态跟踪

在 `ch6` 参考内核上用 `gdb` 断点跟踪，日志见 `reports/ch6-gdb.log`。两个场景：

- **场景 A（启动加载）**：内核加载初始程序 `ch6b_initproc`。
- **场景 B（文件操作）**：`ch6_file0` → 再次 `ch6_file0` → `ch6_file2`，覆盖创建、截断、扩容、读写、链接、`fstat`、删除。

### 2.1 启动加载初始程序

首次命中 `open_file`，调用者就是 `INITPROC`：

```
===== [A1] os::fs::inode::open_file (initial program) =====
#0  os::fs::inode::open_file (name=..., flags=...) at src/fs/inode.rs:106
#1  0x0000000080222052 in os::task::{impl#0}::deref::__static_ref_initialize () at src/task/mod.rs:113
...
$1 = {bits = 0}
$2 = "ch6b_initproc"
```

`flags = {bits = 0}` 即 `RDONLY`，与 `task/mod.rs:113` 的 `open_file("ch6b_initproc", OpenFlags::RDONLY)` 相符。随后进入 `find`（按名字定位），再进入 `read_all`：

```
===== [A3] OSInode::read_all (load whole ELF) =====
#0  os::fs::inode::OSInode::read_all (self=...) at src/fs/inode.rs:42
#1  0x000000008022209c in os::task::{impl#0}::deref::__static_ref_initialize () at src/task/mod.rs:114
$4 = ... {readable: true, writable: false,
          inner: ... value: OSInodeInner {offset: 0, inode: Arc<...> {ptr: ...}}}
```

可以看到新打开对象的 `offset = 0`、`readable=true/writable=false`，与 `RDONLY` 一致；`read_all` 内继续命中 `read_at`：

```
===== [A4] easy_fs::vfs::Inode::read_at =====
#0  easy_fs::vfs::Inode::read_at (self=..., offset=0, buf=...) at src/vfs.rs:277
#1  0x000000008021ac6a in os::fs::inode::OSInode::read_all (self=...) at src/fs/inode.rs:47
$5 = 0
```

这说明**偏移在 `OSInode`、数据在磁盘 inode**：`read_all` 从 0 开始反复调 `read_at`，每次把 `offset` 推进实际读取长度，直到返回 0。

### 2.2 创建文件并扩展目录

`ch6_file0` 以 `CREATE|WRONLY` 打开 `fname`（`flags=513 = 0x201`）：

```
===== [B] Inode::create #1 =====
#0  easy_fs::vfs::Inode::create (self=..., name=...) at src/vfs.rs:136
#1  0x000000008021b06a in os::fs::inode::open_file (name=..., flags=...) at src/fs/inode.rs:114
#2  0x000000008020287a in os::syscall::fs::sys_open (path=..., flags=513) at src/syscall/fs.rs:58
$1 = "fname"

===== [B] Inode::increase_size #1 =====
#0  ... Inode::increase_size (..., new_size=1728, ...) at src/vfs.rs:95
#1  ... Inode::append_dirent (..., inode_id=54, ...) at src/vfs.rs:126
#2  ... create::{closure#2} at src/vfs.rs:157
$2 = 1728
```

新文件 inode 编号 54；`increase_size` 的 `new_size=1728` 说明**是根目录本身在扩容**（1728 = 54×32），即把新目录项追加到目录末尾，而不是文件数据。紧接着写文件：

```
===== [B] Inode::write_at #1 =====
#0  ... Inode::write_at (..., offset=0, buf=...) at src/vfs.rs:282
#1  ... os::fs::inode::{impl#2}::write (...) at src/fs/inode.rs:176
#2  ... sys_write (fd=3, buf=..., len=13) at src/syscall/fs.rs:25
$3 = 0

===== [B] Inode::increase_size #2 =====
#0  ... Inode::increase_size (..., new_size=13, ...) at src/vfs.rs:95
#1  ... write_at::{closure#0} at src/vfs.rs:284
$4 = 13
```

`sys_write` 请求 13 字节，`OSInode::write` 用当前偏移 0 调 `Inode::write_at`；`write_at` 先 `increase_size(13)` 把文件从 0 扩到 13，再落盘。这正是「**底层 `DiskInode::write_at` 只写到 `size`，所以必须先扩容**」的直接证据。

### 2.3 再次打开：`CREATE` 截断

第二次运行 `ch6_file0` 时 `fname` 已存在，`open_file` 走清空分支：

```
===== [B] Inode::clear #1 (CREATE re-opens an existing name) =====
#0  easy_fs::vfs::Inode::clear (self=..., ) at src/vfs.rs:292
#1  ... os::fs::inode::open_file (name=..., flags=...) at src/fs/inode.rs:110
#2  ... sys_open (path=..., flags=513) at src/syscall/fs.rs:58
```

`clear` 的调用点是 `open_file` 中 `if let Some(inode) = ROOT_INODE.find(name) { inode.clear(); ... }`（`inode.rs:110`），说明 **CREATE 打开已有文件会截断**，且清空后 inode 身份不变（随后的 `write_at` 仍复用同一文件）。

### 2.4 硬链接与 `fstat`

`ch6_file2` 先创建 `fname2`（`create #2`，inode 55），再做三次 `link`：

```
===== [B] Inode::link #1 =====
#0  ... Inode::link (self=0x2, old_name=..., new_name=...) at src/vfs.rs:201
#1  ... os::fs::inode::link_file (...) at src/fs/inode.rs:130
#2  ... sys_linkat (old_name=0x430d, new_name=0x4314) at src/syscall/fs.rs:137
```

每次 `link` 都伴随一次根目录 `increase_size`（1792 → 1824 → 1856），说明**每个新名字都占用一条独立目录项**，而不是替换或复制数据。`fstat` 走 `OSInode::stat` → `Inode::stat`：

```
===== [B] Inode::stat #1 =====
#0  ... Inode::stat (self=..., ) at src/vfs.rs:46
#1  ... os::fs::inode::{impl#2}::stat (self=..., ) at src/fs/inode.rs:140
#2  ... sys_fstat (fd=3, st=0xde18) at src/syscall/fs.rs:93
```

`ch6_file2` 断言 `nlink=2 → 4`，与三次 `link` 后目录项数量一致；`stat` 每次都从磁盘 inode 现读，所以能看到后续链接变化。

### 2.5 写入共享数据与删除

写入发生在 `link` 之后，`write_at #3` 的 `increase_size new_size=13` 说明新文件从 0 扩到 13：

```
===== [B] Inode::write_at #3 =====
#0  ... Inode::write_at (..., offset=0, buf=...) at src/vfs.rs:282
#1  ... os::fs::inode::{impl#2}::write (...) at src/fs/inode.rs:176
#2  ... sys_write (fd=3, buf=..., len=13) at src/syscall/fs.rs:25
```

随后四次 `unlink` 对应 `ch6_file2` 删除的四个名字（`fname2/linkname0/linkname1/linkname2` 各一次）：

```
===== [B] Inode::unlink #1 =====
#0  ... Inode::unlink (self=0x313, name=...) at src/vfs.rs:233
#1  ... os::fs::inode::unlink_file (...) at src/fs/inode.rs:135
#2  ... sys_unlinkat (name=0x430d) at src/syscall/fs.rs:144
...
===== [B] Inode::unlink #4 =====
----- ch6_file2 removed all four names, ending session -----
```

四个名字共享 inode 55；只有最后一个 `unlink` 才触发 `clear_inode_data` + `dealloc_inode`，这也解释了为什么 `ch6_file2` 每删一个名字后仍能用其他名字读到相同内容。

### 2.6 跟踪结论

- **身份与偏移分离**：`find`/`stat` 只读磁盘 inode；`offset` 只存在于 `OSInodeInner`，`read_all`/`read`/`write` 按实际长度推进。
- **目录项与硬链接**：`create`/`link` 通过 `append_dirent` 增加目录项（必要时扩展目录），`nlink` 与目录项数量一致；`Arc` 数量与 `nlink` 无关。
- **扩容与回收**：`write_at` 先 `increase_size` 再写；`clear`/最后一次 `unlink` 通过 `clear_inode_data` 回收直接与间接块。
- **`CREATE` 语义**：已存在文件被截断而不替换 inode。

---

## 3. 独立实现对比（`ch6-api`）

`ch6-api` 在相同的磁盘布局与辅助函数之上完成 14 处接口，总体算法与 `ch6` 参考实现一致。

### 3.1 相同点

- **锁与缓存顺序**：都在一次 `fs` 锁内完成 `create/link/unlink` 的查找与修改；都先把目录项写好，再单独加锁写目标 inode 的 `nlink`；都在返回前 `block_cache_sync_all()`。
- **目录项复用**：复用 `append_dirent`（扫描空槽，必要时 `increase_size`），`create` 先查重再分配 inode。
- **链接计数**：`link` 用 `checked_add`、`unlink` 用 `checked_sub`，并拒绝非普通文件；`nlink == 0` 时回收数据块与 inode 编号。
- **打开语义**：`open_file` 的四种分支与参考一致（含「CREATE 遇到已存在即截断」）。
- **`stat`**：都用 `fs.get_disk_inode_id(block_id, offset)` 求编号，读 `nlink` 与 `is_dir`，不使用 `Arc::strong_count`。

### 3.2 不同点

| 方面 | 参考 `ch6` | `ch6-api` |
| --- | --- | --- |
| `read_all` 读缓冲 | `Vec::with_capacity(512)` + `resize`（堆） | `[0u8; 512]` 栈数组 |
| `open_file` 结构 | `if let ... else` | `if/else` + `match` 风格等价 |
| `ls` 局部变量 | `dirent` 在循环内声明 | 循环外声明一次，语义相同 |
| `link/unlink` 注释与借用 | 简洁注释 | 显式标注「同块不可重入」与 `checked_*` 意图 |

差异都属实现风格与内存来源，不改变磁盘格式、目录项顺序、`nlink` 语义或回收时机，因此验收结果与参考一致。

### 3.3 `create` 查重与死锁规避

`create` 必须在分配 inode 前确认同名不存在。若调用公开的 `self.find()`，会再次 `self.fs.lock()` 造成不可重入死锁。`ch6-api` 与参考一样，在**已持有 `fs` 锁**的 `read_disk_inode` 闭包里直接用 `find_inode_id` 完成查重，再分配与插入。`link` 的旧名/新名查找同理一次完成。

### 3.4 验收

`reports/ch6-api-impl-run.log`（`make run BASE=2`）：

- `ch6b_usertest`：`Basic usertests passed!`（含 `ch6b_filetest_simple` 的 `file_test passed!`、`ch6b_cat`）。
- `ch6_usertest`：`ch6 Usertests passed!`，其中
  - `ch6_file0`：`Test file0 OK!`（创建/写/读）
  - `ch6_file1`：`Test fstat OK!`（`mode`、`nlink`）
  - `ch6_file2`：`Test link OK!`（`nlink` 2→4→3→1、`ino` 稳定）
  - `ch6_file3`：`Test mass open/unlink OK!`（10×50 次约 127 KB 写入，走到二级间接块后再整体回收）

`make build MODE=debug BASE=2` 亦通过，`ch6-api` 的 `fs-img` 在实现后能正常由 `easy-fs-fuse` 生成。

---

## 4. 主要问题与解决思路

1. **文件系统锁不可重入**：`self.fs` 是 `spin::Mutex`，`create/link/unlink` 若调用 `self.find()` 会在持锁时再次加锁而死锁。
   解决：在一次 `fs` 锁内用 `find_inode_id`/`find_dirent` 完成全部查找；日志中 `create` 的调用栈只经过 `open_file`，没有嵌套的 `find` 公开调用。
2. **目录与目标 inode 可能同块**：同时持有同一块的两个缓存守卫会死锁。
   解决：读目标 `nlink` 的守卫用完即释放，再 `modify_disk_inode` 改目录项，最后再单独加锁写 `nlink`（见 GDB `link #1` 的调用顺序）。
3. **`write_at` 不自动扩容**：底层 `DiskInode::write_at` 以 `size` 为界，直接写新数据会写空。
   解决：`Inode::write_at` 先 `increase_size(offset + buf.len())`，再写、再同步。GDB 中 `increase_size new_size=13` 紧跟在 `write_at` 前印证。
4. **读取需要可变 `UserBuffer`**：`File::read` 要逐片写入用户缓冲区。
   解决：实现签名为 `fn read(&self, mut buf: UserBuffer)`（与 trait 兼容），`buf.buffers.iter_mut()` 逐片 `read_at`，按实际长度推进偏移。
5. **`nlink` 不能用 `Arc` 强引用判断**：`Arc` 数量受句柄影响，且最后删除要立即回收。
   解决：`stat`/`unlink` 一律读磁盘 `DiskInode::nlink`；`unlink` 减到 0 才 `clear_inode_data` + `dealloc_inode`。`ch6_file2` 的 `nlink` 变化与四个 `unlink` 一一对应。
6. **`CREATE` 截断但保留 inode**：清空必须保留编号与链接关系。
   解决：`open_file` 对已存在文件只调 `inode.clear()`，不重新 `create`；`clear` 只回收数据块，不动 inode 位图。
7. **二级间接块路径**：`ch6_file3` 单次写入远超一级间接容量。
   解决：`increase_size`/`clear_size` 委托给已提供的 `DiskInode` 实现，测试通过说明二级间接索引的分配与回收均正确。

---

## 5. 附录

- GDB 日志：`reports/ch6-gdb.log`；脚本：`reports/ch6-gdb.cmd`
- 验收运行日志：`reports/ch6-api-impl-run.log`
- 分析参考：`rcore-ch6-analyze.md`；接口契约：`rcore-ch6-api.md`
- 独立实现分支：`ch6-api`，提交 `feat(ch6-api): implement file, directory and hard-link management`
- API 配套的 `exec` 已由课题提供栈/堆边界重置，属提供实现，未计入本次 14 处 TODO。
