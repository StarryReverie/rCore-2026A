# rCore ch4 实验报告：Sv39 页表与地址空间的静态分析、GDB 动态跟踪与独立实现

本报告对应第 4 章。分析对象是 `ch4` 分支的参考实现（`make build MODE=debug BASE=2`
构建的调试内核）；独立实现位于 `ch4-api` 分支的
[`os/src/mm/page_table.rs`](../os/src/mm/page_table.rs) 和
[`os/src/mm/memory_set.rs`](../os/src/mm/memory_set.rs)。报告包含三部分：静态分析与动态
跟踪、独立实现与对比、主要问题与解决思路。

## 0. 实验环境与构建

| 项 | 版本 / 值 |
| --- | --- |
| 宿主 | NixOS，使用仓库 `flake.nix` 提供的 dev shell（direnv 自动进入） |
| Rust | `rustc 1.80.0-nightly`，target `riscv64gc-unknown-none-elf` |
| QEMU | 7.0.0（flake 固定的 `nixpkgs-qemu7`） |
| GDB | GNU gdb 17.2，多架构，`set architecture riscv:rv64` |
| 固件 | `bootloader/rustsbi-qemu.bin` |
| 参考实现 | `ch4` 分支 |
| 独立实现 | `ch4-api` 分支（仅 `mm/page_table.rs`、`mm/memory_set.rs`） |
| 应用 | `BASE=2`，21 个应用（`app_0..app_20`，见 `user/build/bin/`） |

调试构建、GDB 服务端与连接：

```bash
cd os
make build MODE=debug BASE=2                 # 带调试信息、未优化的内核
make gdbserver MODE=debug BASE=2             # 以 -s -S 启动 QEMU，等待 GDB
# 另一个终端：
gdb -nx -q target/riscv64gc-unknown-none-elf/debug/os
```

NixOS 没有 `riscv64-unknown-elf-gdb`，框架已支持 `GDB ?= riscv64-unknown-elf-gdb`，
调试时使用多架构 `gdb`。测试应用顺序（`BASE=2`）：

```
ch2b_bad_address ch2b_bad_instructions ch2b_bad_register ch2b_hello_world
ch2b_power_3 ch2b_power_5 ch2b_power_7 ch3b_yield0 ch3b_yield1 ch3b_yield2
ch3_sleep1 ch3_sleep ch3_trace ch4b_sbrk ch4_mmap0 ch4_mmap1 ch4_mmap2
ch4_mmap3 ch4_trace1 ch4_unmap2 ch4_unmap
```

本次 debug 构建的关键符号地址（取自 `rust-nm`）：

| 符号 | 地址 | 说明 |
| --- | --- | --- |
| `skernel` / `stext` / `_start` | `0x80200000` | 内核起点 / `.text` 起点 |
| `strampoline` / `__alltraps` | `0x80201000` | 跳板代码起点，`strampoline` 在 `.text` 内 |
| `__restore` | `0x80201060` | 恢复 Trap 上下文并 `sret` |
| `trap_handler` | `0x8020f3e2` | Rust Trap 处理函数 |
| `rust_main` | `0x802113e8` | Rust 启动入口 |
| `__switch` | `0x802182ae` | 任务上下文切换汇编 |
| `etext` / `srodata` | `0x80225000` | `.text` 终点 / `.rodata` 起点 |
| `erodata` / `sdata` | `0x8022c000` | `.rodata` 终点 / `.data` 起点 |
| `edata` / `sbss_with_stack` | `0x8089a000` | `.data` 终点 / BSS（含启动栈）起点 |
| `boot_stack_lower_bound` | `0x8089a000` | 启动栈下界 |
| `boot_stack_top` / `sbss` | `0x808aa000` | 启动栈顶 / BSS 符号起点 |
| `ebss` / `ekernel` | `0x828ab000` | BSS 终点 / 内核占用物理内存终点 |

证据文件：

- [`reports/ch4-gdb.cmd`](ch4-gdb.cmd)：本次 GDB 命令脚本。
- [`reports/ch4-gdb.log`](ch4-gdb.log)：本次完整 GDB 会话日志（765 行）。
- [`reports/ch4-qemu-run.log`](ch4-qemu-run.log)：参考实现 release 下的运行输出。
- `reports/ch4-api-impl-run.log`：独立实现 `ch4-api` 的验收输出。

## 1. 静态分析

### 1.1 地址、页号与页内偏移

[`os/src/mm/address.rs`](../os/src/mm/address.rs) 用四个 newtype 区分地址与页号，并在编译期
锁定 Sv39 的位宽：

- `PhysAddr` 取 56 位、`PhysPageNum` 取 44 位（`PA_WIDTH - 12`）；
- `VirtAddr` 取 39 位、`VirtPageNum` 取 27 位（`VA_WIDTH - 12`），页大小 `PAGE_SIZE = 4 KiB`。

关键操作：

- `floor()` = `addr / PAGE_SIZE`，`ceil()` = `(addr - 1 + PAGE_SIZE) / PAGE_SIZE`，
  `page_offset()` = 低 12 位。页内偏移在地址转换中必须保留，这是 `translate_user` 与
  `translated_byte_buffer` 正确的前提。
- `VirtPageNum::indexes()` 从高到低取三级页表索引，各 9 位：

  ```rust
  let mut vpn = self.0;
  for i in (0..3).rev() { idx[i] = vpn & 511; vpn >>= 9; }
  ```

  它把 Sv39 的三级遍历（`root -> 中间 -> 末级`）压缩成一个 3 元素数组。
- `From<VirtAddr> for usize` 对第 38 位置位的地址做符号扩展，使 `TRAMPOLINE`
  （`0xffff_ffff_ffff_f000`）等高位地址成为规范地址。
- `PhysPageNum::get_pte_array()` 把物理页视为 `[PageTableEntry; 512]`，
  `get_bytes_array()` 视为 `[u8; 4096]`，这是页表遍历和数据访问访问物理内存的唯一入口。
- `VPNRange`（`SimpleRange<VirtPageNum>`）+ `StepByOne` 提供左闭右开的页号迭代，
  `MapArea` 的映射/解除映射都依赖它。

### 1.2 Sv39 页表与页表项

[`os/src/mm/page_table.rs`](../os/src/mm/page_table.rs) 定义页表和转换逻辑：

```rust
pub struct PageTable { root_ppn: PhysPageNum, frames: Vec<FrameTracker> }
```

- `PageTableEntry` 编码为 `bits = ppn << 10 | flags`，`ppn()` 取第 10..54 位，
  `flags()` 取低 8 位。`PTEFlags` 中 `V/R/W/X/U/G/A/D` 与 Sv39 位定义一致。
- `new()` 分配根页帧并放进 `frames`；`from_token()` 只按 `satp` 低 44 位构造查询视图，
  `frames` 为空、不取得页表页帧所有权。二者的区别正是“拥有”与“借用”页表的区别。
- 三级遍历由两个辅助函数承担：
  - `find_pte_create`：中间级 PTE 无效时 `frame_alloc()` 分配页表页，写
    `PageTableEntry::new(ppn, V)` 并压入 `self.frames`，返回末级 PTE；分配失败返回 `None`。
  - `find_pte`：只读下降，中间级无效立即返回 `None`，末级 PTE 不论 `V` 与否都返回。
- `map()` 用 `find_pte_create` 建映射，写入 `flags | V`；已有有效映射时返回 `None`，
  避免覆盖。中间页表页归 `PageTable` 所有，数据页归调用方，所有权严格分离。
- `unmap()` 取末级 PTE，断言有效后置空，只解除映射、不回收数据页。
- `translate()` 返回末级 PTE 的副本，保留原标志，调用方自行判断 `V`。
- `translate_user()` 做三重检查：地址需为规范 Sv39 地址（用 `VirtAddr` 往返比较）、
  末级 PTE 必须含 `V | U | permission`、返回值保留页内偏移。
- `translated_byte_buffer()` 按页推进，逐页取 `ppn` 并切片，页边界时切到页尾；
  虚拟连续而物理不连续的缓冲区因此被拆成多个物理切片，字节顺序不变。
- `token()` = `8 << 60 | root_ppn`，其中 `8` 是 Sv39 模式，写入 `satp` 后即激活该页表。

### 1.3 逻辑段与地址空间

[`os/src/mm/memory_set.rs`](../os/src/mm/memory_set.rs) 用逻辑段组织映射：

- `MapType::Identical` 令 `ppn = vpn`，不持有数据页；`MapType::Framed` 为每页
  `frame_alloc()` 并记录 `data_frames: BTreeMap<VirtPageNum, FrameTracker>`。
- `MapPermission` 的 `R/W/X/U` 与 `PTEFlags` 同位（`bits` 直接转换）。
- `MapArea::{map_one, map, unmap_one, unmap, shrink_to, append_to, copy_data}` 是已提供的
  配套实现；它们通过 `PageTable::{map, unmap, translate}` 操作页表。`copy_data` 假设页帧已
  清零，把 ELF 段的数据按页拷入。
- `MemorySet { page_table, areas }` 中 `page_table` 描述实际映射，`areas` 描述由逻辑段
  管理的部分；`has_mapped_pages` / `has_unmapped_pages` 用 `translate` 判断区间映射状态，
  `mmap`/`munmap` 正是靠它们在建立/解除前做整体冲突检查。
- `KERNEL_SPACE` 由 `new_kernel()` 初始化，映射关系为：

  | 区间 | 方式 | 权限 |
  | --- | --- | --- |
  | `[stext, etext)` | Identical | RX |
  | `[srodata, erodata)` | Identical | R |
  | `[sdata, edata)` | Identical | RW |
  | `[sbss_with_stack, ebss)` | Identical | RW |
  | `[ekernel, MEMORY_END)` | Identical | RW |
  | `TRAMPOLINE` | `strampoline` 物理页 | RX |

  跳板页由 `map_trampoline()` 单独映射，不进入 `areas`；所有内核映射都不设 `U`。
- `from_elf()` 为每个应用建立独立地址空间：ELF 的 `PT_LOAD` 段按文件权限 `Framed` 映射
  并 `copy_data`；最高段之后隔一个未映射保护页放 `USER_STACK_SIZE` 的用户栈；栈顶放一个
  长度为零的 RWU 堆段供 `sbrk` 调整；`[TRAP_CONTEXT_BASE, TRAMPOLINE)` 用独立页帧映射
  为内核可访问的 RW（无 `U`）；最后映射跳板页。返回 `(MemorySet, user_stack_top, entry)`。
- `insert_framed_area` 为内核栈或匿名映射分配零初始化页帧；`remove_framed_area` 以完整
  逻辑段为单位解除映射并释放数据页；`shrink_to`/`append_to` 调整堆段，分别截掉/追加尾页。
- `kernel_stack_position(app_id)` = `TRAMPOLINE - app_id * (KERNEL_STACK_SIZE + PAGE_SIZE)`，
  每个任务的内核栈下方留一个保护页，栈空间不落入 `areas`（由任务模块通过
  `insert_framed_area` 动态加入 `KERNEL_SPACE`）。

### 1.4 页帧所有权与回收

[`os/src/mm/frame_allocator.rs`](../os/src/mm/frame_allocator.rs) 用 `FrameTracker` RAII
管理物理页：`frame_alloc()` 返回清零的 `FrameTracker`，其 `Drop` 调用 `frame_dealloc`
归还页帧。由此形成清晰的所有权链：

- 根页表页和中间页表页 -> `PageTable::frames`；
- 数据页 -> `MapArea::data_frames`（`Identical` 段不持有）；
- Trap 上下文页 PPN -> `TaskControlBlock::trap_cx_ppn`；
- `from_token` 的查询视图不持有任何页帧。

`shrink_to`/`remove_framed_area`/`unmap_one` 删除 `data_frames` 条目即触发回收，页表项的
清除与页帧回收因此保持同步。

### 1.5 调用方：任务管理与系统调用

- [`os/src/task/task.rs`](../os/src/task/task.rs)：`TaskControlBlock::new` 调用
  `MemorySet::from_elf`，从返回的 `memory_set` 中取出 Trap 上下文页的 `ppn`，再通过
  `KERNEL_SPACE` 的 `insert_framed_area` 分配内核栈，最后把 `KERNEL_SPACE` 的 token
  填入 `TrapContext`。`mmap`/`munmap`/`change_program_brk` 分别转调
  `insert_framed_area`/`remove_framed_area`/`append_to`·`shrink_to`。
- [`os/src/syscall/fs.rs`](../os/src/syscall/fs.rs)：`sys_write` 用
  `translated_byte_buffer(current_user_token(), buf, len)` 把用户缓冲区变成物理切片后输出。
- [`os/src/syscall/process.rs`](../os/src/syscall/process.rs)：`sys_get_time` 与 `sys_trace`
  先用 `PageTable::from_token` 建立查询视图，再调用 `translate_user` 检查 `R`/`W` 权限；
  `sys_mmap`/`sys_munmap`/`sys_sbrk` 走上面的任务接口。

### 1.6 启动与 Trap/跳板页

[`os/src/main.rs`](../os/src/main.rs) 的 `rust_main` 依次执行 `clear_bss ->
kernel_log_info -> mm::init() -> remap_test() -> trap::init() -> run_first_task()`。
其中 `mm::init()` 初始化内核堆与页帧分配器，并调用 `KERNEL_SPACE.activate()` 写 `satp`；
`remap_test()` 断言 `.text`、`.rodata` 不可写、`.data` 不可执行，验证最小权限映射。

[`os/src/linker.ld`](../os/src/linker.ld) 把 `strampoline`（`trap.S` 的 `__alltraps`/`__restore`）
放在 `.text` 内并页对齐（`0x80201000`）。跳板页被同时映射到内核和各用户页表的
`TRAMPOLINE`（RX、无 U），因此切换地址空间后仍能执行陷阱处理代码：

- 用户态陷入时 `stvec` 指向 `TRAMPOLINE`，`__alltraps` 先 `csrrw` 换上内核栈、保存
  `TrapContext`，再跳入 `trap_handler`；
- `trap_handler` 通过 `current_trap_cx()` 读取上下文并分发（如 `UserEnvCall` -> `syscall`）；
- 返回时 `__restore` 从 Trap 上下文取用户 token，写 `satp` 并 `sfence.vma`，再 `sret`。

需要注意的是，`MemorySet::activate()` 只在启动时调用一次；运行期的页表切换由 `__restore`
写 `satp` 完成，这是后文 GDB 观察到的 `activate` 只命中一次的原因。

## 2. GDB 动态跟踪

### 2.1 连接与命令

按 [`reports/ch4-gdb.cmd`](ch4-gdb.cmd) 执行：先 `make gdbserver MODE=debug BASE=2`，
再用多架构 `gdb` 连接 `localhost:1234`，打开日志后设置断点。为保证 Rust 参数可读，
断点落在“使用参数之后”的源码行（如 `page_table.rs:144`），并对高频断点在命中若干次后
`disable $_hit_bpnum`，以免遍历物理内存恒等映射时产生过多停顿。

### 2.2 调用链 1：`mm::init` 建立内核地址空间

`MemorySet::new_kernel` 在 `KERNEL_SPACE` 首次解引用时经 `spin::once` 执行。调用栈为
`new_kernel -> deref::__static_ref_initialize -> FnOnce::call_once`（源码推导一致）。

`PageTable::map` 前几次命中（实际观察）：

| 次数 | vpn | ppn | flags | 来源 |
| --- | --- | --- | --- | --- |
| 1 | `0x7ffffff` | `0x80201` | `0xa` (R\|X) | `map_trampoline`：`TRAMPOLINE -> strampoline(0x80201000)` |
| 2 | `0x80200` | `0x80200` | `0xa` | `.text` 恒等映射首 4 KiB |
| 3 | `0x80201` | `0x80201` | `0xa` | `.text` 第二页 |
| 4 | `0x80202` | `0x80202` | `0xa` | `.text` 第三页 |

第 2~4 次的调用栈为 `map -> MapArea::map_one -> MemorySet::push -> new_kernel`，说明
「逻辑段逐页调用 `PageTable::map`」的组织方式。第 1 次 `vpn=0x7ffffff` 是 `TRAMPOLINE`
取低 39 位后的页号，映射到 `strampoline`。

`MemorySet::activate` 命中一次，调用栈为 `os::mm::init -> rust_main`。写 `satp` 前后：

- 前：`satp = 0x0`；
- 后：`satp = 0x80000000000828ab`，即 模式 `8`(Sv39) + `root_ppn = 0x828ab`。

根页表物理地址 `0x828ab000` 恰为 `ekernel`（`0x828ab000`），也就是页帧分配器的第一个
空闲页帧，符合「`init_frame_allocator` 从 `ekernel.ceil()` 开始分配」的源码推导。

`remap_test` 时 `satp` 仍为 `0x80000000000828ab`，输出 `remap_test passed!`，证明
`.text`/`.rodata` 不可写、`.data` 不可执行。

### 2.3 调用链 2：`TaskControlBlock::new -> MemorySet::from_elf`

`from_elf` 首次命中时调用栈为 `from_elf -> TaskControlBlock::new(app_id=0) -> ...`。
在用户栈构造点（`memory_set.rs:210`）观察到：

- `max_end_vpn = 0x11`（最高程序段结束虚拟页，VA `0x11000`）；
- `user_stack_bottom = 0x12000`，`user_stack_top = 0x14000`。

由此可验证布局：保护页位于 `0x11000..0x12000`，用户栈恰好 `0x12000..0x14000`
（`USER_STACK_SIZE = 0x2000`），堆起点即 `user_stack_top`。

`insert_framed_area` 的前几次命中来自任务内核栈：

| 次数 | start_va | end_va | permission |
| --- | --- | --- | --- |
| 1 | `0x7fffffd000` | `0x7ffffff000` | `bits=6` (R\|W) |
| 2 | `0x7fffffa000` | `0x7fffffc000` | `bits=6` |

它们对应 `kernel_stack_position(0)` 与 `kernel_stack_position(1)`：栈顶贴近
`TRAMPOLINE`，每级相差 `KERNEL_STACK_SIZE + PAGE_SIZE = 0x3000`（含保护页）。内核栈
被加入 `KERNEL_SPACE`，权限不带 `U`。

### 2.4 调用链 3：`sys_write -> translated_byte_buffer`

`sys_write` 命中 `fd=1, buf=0xd000, len=23`，调用栈为
`sys_write -> syscall(syscall_id=64) -> trap_handler`。随后 `translated_byte_buffer`
观察到：

- `token = 0x800000000008295a`（`8<<60 | ppn`，标识某个用户页表）；
- `ptr = 0xd000`，`len = 23`；
- 调用栈 `translated_byte_buffer -> sys_write(fs.rs:13) -> syscall -> trap_handler`。

`ptr` 与 `len` 说明缓冲区完全落在一页内；`translated_byte_buffer` 使用传入的用户 token
查询映射，把 `[0xd000, 0xd017)` 切成一个物理切片。若缓冲区跨页，循环会按 `end_va` 逐页
切片，页内偏移与物理不连续由 `ppn.get_bytes_array()[offset..]` 自然处理。

### 2.5 调用链 4：`sys_mmap -> insert_framed_area`

`sys_mmap` 观察到 `ch4_mmap0` 的两次调用（实际观察）：

- `start = 0x10000000, len = 4096, prot = 3`（R\|W）；
- `start = 0x10000000, len = 4096, prot = 1`（R）。

`prot` 是低位的 `R/W/X`，任务层再左移一位并补 `U`，因此页表权限为 RWU / RU。之后
`insert_framed_area` 为该区间分配零页并建立 Framed 映射。`ch4_mmap1` 对只读页写入时
触发 `StorePageFault`，内核打印 `PageFault ... bad addr = 0x10000000` 并杀掉该应用，
说明权限检查确实落在页表项上。

### 2.6 调用链 5：`sys_munmap -> remove_framed_area`

`ch4_unmap2` 的错误参数被任务层拦截（实际观察）：

- `start = 0x10000000, len = 4097`：`end.ceil()` 后超出目标段，`has_unmapped_pages`
  为真，返回失败；
- `start = 0x10000001, len = 4095`：起始地址未页对齐，`sys_munmap` 直接返回 `-1`；
- `start = 0x10000000, len = 4096`：合法，进入 `remove_framed_area`。

`remove_framed_area` 观察到 `start = 0x10000, end = 0x10001`（页号，对应
`0x10000000` 的 1 页），调用栈
`remove_framed_area -> TaskControlBlock::munmap -> munmap_current -> munmap_current_task`。
日志中还能看到 `remove_framed_area` 内部闭包被逐项调用（`impl#0::closure#0`），对应
`areas.iter().position(...)` 的线性查找；随后 `MapArea::unmap` 清除 PTE 并删除
`data_frames` 条目，触发 `frame_dealloc`。

### 2.7 调用链 6：`sys_sbrk -> append_to / shrink_to`

`ch4b_sbrk` 的堆操作（实际观察）：

- `sys_sbrk(size=0)` -> `append_to(start=0x14000, new_end=0x14000)`，空操作；
- 扩容：`new_end` 依次到 `0x15000`、`0x16000`、`0x1f000`，调用栈
  `append_to -> change_program_brk -> change_current_program_brk -> change_program_brk`；
- 缩容：`sys_sbrk(size=-4096)` 直到 `size=-45056`，`shrink_to(start=0x14000, new_end=0x14000)`，
  截掉的页面 `unmap_one` 并回收页帧。

`heap_bottom = 0x14000`（即 `user_stack_top`），与源码中 `heap_bottom = user_sp` 一致。
缩容后应用再写已释放页会触发 `PageFault ... bad addr = 0x14000`，证明页帧确已回收、
映射确已撤销。

### 2.8 调用链 7：`sys_trace -> translate_user`

`sys_trace` 观察到两类请求（实际观察）：

- `trace_request=0/1, id=0x139d7(80343), data=0/33`；
- `trace_request=0, id=0x7fffffffffffffff`（非法地址）。

对应的 `translate_user`：

| 来源 | addr | permission | 结果 |
| --- | --- | --- | --- |
| `sys_trace` 读 | `0x139d7` | `bits=2` (R) | 映射有效且可读 -> 返回物理地址 |
| `sys_trace` 写 | `0x139d7` | `bits=4` (W) | 映射有效且可写 -> 返回物理地址 |
| `sys_get_time` | `0x13d88` 等 | `bits=4` (W) | 映射有效且可写 -> 返回物理地址 |
| `sys_trace` 非法 | `0x7fffffffffffffff` | `bits=2` | 非规范地址 -> `None`，返回 `-1` |

调用栈同时出现 `sys_trace(process.rs:80)` 与 `sys_get_time(process.rs:45)`，说明同一
`translate_user` 同时服务多条系统调用路径。日志中 `translate_user` 的 `self` 是
`0xffff...` 形式的临时查询视图，即 `PageTable::from_token` 的 `frames` 为空视图。

### 2.9 观察结果汇总

| 调用链 | 关键观察 | 与源码推导的一致性 |
| --- | --- | --- |
| 内核地址空间 | `satp 0x0 -> 0x80000000000828ab`，根页表在 `ekernel` | 一致 |
| `from_elf` | `max_end_vpn=0x11`，栈 `0x12000..0x14000`，保护页 `0x11000` | 一致 |
| 内核栈 | `insert_framed_area` 权限 `R｜W`，间隔 `0x3000` | 一致 |
| 用户缓冲 | `translated_byte_buffer` 用用户 token 查询、保留偏移 | 一致 |
| `mmap`/`munmap` | 权限来自 `prot`，错误参数返回 `-1`，只读写触发 PageFault | 一致 |
| `sbrk` | 堆从 `0x14000` 增长到 `0x1f000`，可缩回并触发 PageFault | 一致 |
| `trace` | `translate_user` 检查 `V/U/R/W`，非法地址返回 `-1` | 一致 |

## 3. 独立实现与对比

### 3.1 实现组织

`ch4-api` 的 11 处 TODO 全部落在两个 `mm` 源文件。实现组织为：

- `page_table.rs`：`find_pte_create` / `find_pte` 两个私有辅助函数；公开
  `map`、`unmap`、`translate`、`translate_user`，以及自由函数 `translated_byte_buffer`。
- `memory_set.rs`：`push` / `map_trampoline` / `new_kernel` 三个私有/公开辅助；
  `KERNEL_SPACE` 经 `new_kernel()` 初始化；公开 `insert_framed_area`、`from_elf`、
  `remove_framed_area`、`shrink_to`、`append_to`。

### 3.2 与参考实现的相同点

- 三级遍历、页表页分配（`frames` 持有）、`V` 位设置、已有映射拒绝的语义完全一致；
- `KERNEL_SPACE` 的五段恒等映射与跳板页权限、`from_elf` 的段/保护页/栈/空堆/Trap 上下文
  布局、`shrink_to`/`append_to` 的页范围调整都与参考实现相同；
- 均复用已提供的 `MapArea` 方法，不复制其逻辑。

### 3.3 与参考实现的不同点

| 方面 | 参考实现 | 本实现 |
| --- | --- | --- |
| 三级遍历写法 | `enumerate` + `result` 变量 + `break` | `for level in 0..3` 提前 `return`，末尾 `unreachable!()` |
| 命名 | `v`（缓冲区）、`data`/`push` 等 | `buffers`、`mapped` 等，中文注释说明约束 |
| `translate_user` 规范地址 | 同样用 `VirtAddr` 往返比较 | 相同思路，注释显式说明「截断即拒绝」 |
| `push` 失败语义 | 映射失败仍把 `MapArea` 压入 `areas` | 先沿用同一语义并在注释中标注，作为待改进点 |
| 日志 | `info!` 打印各段区间 | 保留同样信息，格式对齐便于阅读 |

这些差异不影响接口契约与可观察行为；`ch4-api` 的验收输出（`remap_test passed!`、所有
`ch4_mmap*`/`ch4_unmap*`/`ch4b_sbrk`/`ch4_trace1` 断言通过）与参考实现一致。

### 3.4 验证

```bash
cd os
make build CHAPTER=4 BASE=2
make run   CHAPTER=4 BASE=2
```

结果：`remap_test passed!`，`Test 04_1/04_4/04_5/04_6 OK`，`Test sbrk almost OK`（写已释放
页触发 PageFault），`Test trace_1 OK`，回归的 `ch2`/`ch3` 用例通过，最后
`All applications completed!`。

## 4. 主要问题与解决思路

### 4.1 NixOS 没有 `riscv64-unknown-elf-gdb`

现象：`make gdbserver` 后无法用 `riscv64-unknown-elf-gdb` 连接。
排查：NixOS 只提供多架构 `gdb`。
解决：`os/Makefile` 已支持 `GDB ?= riscv64-unknown-elf-gdb`，调试时用
`gdb -nx -q ...` 并 `set architecture riscv:rv64`。

### 4.2 `ch4-api` 分支名导致 `CHAPTER` 自动探测失败

现象：`make run BASE=2` 在 `ch4-api` 上把章节解析成 `4-api`，用户应用构建报错。
解决：显式传 `CHAPTER=4`，即 `make run CHAPTER=4 BASE=2`。

### 4.3 GDB 在函数入口读到未就绪的参数

现象：在 `PageTable::map` 入口直接 `info args`，`vpn/ppn/flags` 是上一次调用的残值。
原因：断点落在序言之前，参数尚未写入栈槽。
解决：把断点改到「参数使用之后」的源码行（如 `page_table.rs:144`），或先 `next`。
本报告统一采用行断点，避免 `next` 在批处理命令列表中提前结束会话。

### 4.4 高频断点拖慢整机运行

现象：`PageTable::map` 在 `[ekernel, MEMORY_END)` 恒等映射时要命中上万次。
解决：脚本中先用计数器只打印前若干次，命中阈值后 `disable $_hit_bpnum`，既保留证据又避免
无意义的停顿。

### 4.5 `from_elf` 的 `elf_data` 过大无法打印

现象：`info args` 对 `elf_data` 报 `value requires 315392 bytes ...`。
原因：GDB `max-value-size` 限制。
解决：该信息对分析不重要，`from_elf` 只需调用栈与用户栈边界；GDB 对该错误不会中断会话，
日志仍完整。必要时可 `set max-value-size unlimited` 或只打印指针。

### 4.6 debug 构建下用户程序出现 `write_volatile` 前置条件

现象：`make run MODE=debug BASE=2` 在 `ch4b_sbrk` 附近打印
`Panicked at library/core/src/panicking.rs:220, unsafe precondition(s) violated:
ptr::write_volatile requires that the pointer argument is aligned and non-null`，
但内核继续调度，最终仍 `All applications completed!`。
分析：这是 debug 构建启用的 `core` 前置条件检查，与本章页表映射本身的正确性无关；
release 验收输出正常（见 `ch4-qemu-run.log`）。GDB 跟踪使用 debug 内核，便于符号解析，
功能验收仍以 release 为准。

### 4.7 `push` 的部分失败未回滚（尚未解决）

`MapArea::map` 逐页映射，若中途 `frame_alloc` 失败，参考实现与本实现都会把（部分映射的）
`MapArea` 记入 `areas`，导致逻辑段记录与页表状态可能不一致。该路径在现有测试中不会触发
（内存充足），但它是一个真实的健壮性缺口：更稳妥的做法是在 `push` 内记录已成功映射的页，
失败时逐页 `unmap` 并回收，再返回 `None`。此处如实记录，留待后续改进。
