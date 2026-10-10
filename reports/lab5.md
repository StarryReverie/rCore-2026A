# 第 5 章 进程管理与 stride 调度 实验报告

## 0. 实验对象与环境

| 项 | 内容 |
| --- | --- |
| 分析对象 | `ch5` 分支参考实现（进程生命周期 + stride 调度） |
| 独立实现 | `ch5-api` 分支，`os/src/task/{task,manager,processor,mod}.rs` 十一处 TODO |
| 构建 | `make build BASE=2`；GDB 跟踪用 `make build MODE=debug BASE=2`（64 KiB 内核栈） |
| 运行/验收 | `make run BASE=2`，在用户 shell 内运行 `ch5_usertest`、`ch5b_usertest` |
| 调试器 | NixOS 无 `riscv64-unknown-elf-gdb`，使用多架构 `gdb` 17.2 + `make gdbserver MODE=debug BASE=2`，命令加 `GDB=gdb` |
| GDB 日志 | `reports/ch5-gdb.log`（本报告第三部分证据来源） |

GDB 会话的固定设置：

```gdb
set pagination off
set architecture riscv:rv64
set logging file ../reports/ch5-gdb.log
set logging enabled on
target remote localhost:1234
```

---

## 1. 静态分析

### 1.1 数据结构

`TaskControlBlock` 把进程拆成「不可变身份」与「可变运行状态」两部分：

```rust
pub struct TaskControlBlock {
    pub pid: PidHandle,          // PID；Drop 时归还 PidHandle
    pub kernel_stack: KernelStack, // 内核栈；Drop 时解除内核映射并归还编号
    inner: UPSafeCell<TaskControlBlockInner>,
}
```

`TaskControlBlockInner` 中与本章实验直接相关的字段：

- `trap_cx_ppn`：`TRAP_CONTEXT_BASE` 所在物理页，指向本进程的 `TrapContext`。
- `task_cx`：本进程在内核中的切换现场（`TaskContext`）。
- `task_status`：`UnInit / Ready / Running / Zombie`，只描述「是否有调度资格」，与地址空间、CPU 现场分开维护。
- `stride` / `prio`：stride 调度属性。
- `memory_set`：用户地址空间（含用户栈、堆、`TrapContext`、用户程序）。
- `parent: Option<Weak<..>>`、`children: Vec<Arc<..>>`：父子关系用「子→父弱引用、父→子强引用」避免 `Arc` 环。
- `exit_code`：退出码，`Zombie` 期间保存，供父进程 `waitpid` 取走。
- `base_size` / `heap_bottom` / `program_brk`：用户栈顶边界与堆区间，供已有 `sbrk`（`change_program_brk`）使用。

调度侧：

```rust
pub struct TaskManager { ready_queue: VecDeque<Arc<TaskControlBlock>> }
pub struct Processor   { current: Option<Arc<TaskControlBlock>>, idle_task_cx: TaskContext }
```

`Processor.idle_task_cx` 是内核调度循环的控制流，不是用户进程、没有 PID；`IDLE_PID = 0` 是**首个用户进程**（initproc）的 PID，两者不可混淆。

`TaskContext` 与 `TrapContext` 用途不同：

| | `TaskContext` | `TrapContext` |
| --- | --- | --- |
| 内容 | `ra`、`sp`、`s0-s11` | 用户 `x0-x31`、`sstatus`、`sepc`、`kernel_satp`、`kernel_sp`、`trap_handler` |
| 位置 | TCB 内部（`task_cx`） | 用户地址空间 `TRAP_CONTEXT_BASE` 页 |
| 作用 | 内核态进程间切换（保存/恢复调用流） | 内核 ↔ 用户态进出（保存/恢复用户现场） |
| 谁恢复 | `__switch` | `__restore`（经 `trap_return`） |

因此「用户返回现场」在 `TrapContext` 里，内核调用栈（`bt`）不等于用户程序调用栈。

### 1.2 启动与初始进程

`rust_main()`（`os/src/main.rs:113`）依次 `mm::init()` → `task::add_initproc()` → `trap::init()` → `run_tasks()`：

```rust
pub fn add_initproc() { add_task(INITPROC.clone()); }

lazy_static! {
    pub static ref INITPROC: Arc<TaskControlBlock> = Arc::new(
        TaskControlBlock::new(get_app_data_by_name("ch5b_initproc").unwrap()));
}
```

`INITPROC` 是延迟初始化：首次解引用时调用 `TaskControlBlock::new`，得到 PID 0 的 `Ready` 进程，随后 `add_task` 入就绪队列。`new` 从 ELF 构造独立地址空间、分配 PID/内核栈，并写好两套上下文：

- `task_cx = TaskContext::goto_trap_return(kernel_stack_top)`：之后被调度时从内核栈顶进入 `trap_return`，再 `__restore` 到用户态。
- `TrapContext::app_init_context(entry, user_sp, KERNEL_SPACE.token(), kernel_stack_top, trap_handler)`：完整初始化入口、用户栈、用户态权限、内核页表 token、内核栈顶与陷阱入口。

### 1.3 进程创建：fork

`sys_fork`（`os/src/syscall/process.rs:45`）→ `TaskControlBlock::fork`（`os/src/task/task.rs:182`）：

1. `MemorySet::from_existed_user` 深拷贝父地址空间（含用户数据与 `TrapContext`），重新计算 `trap_cx_ppn`；
2. `pid_alloc` + `kstack_alloc` 给子进程分配新 PID 与内核栈；
3. 构造子 TCB：`base_size/heap_bottom/program_brk` 继承父进程，`parent=Weak(self)`，`children` 空，`exit_code=0`，`stride=0`，`prio=16`；
4. `parent_inner.children.push(child)`；
5. 修正子进程 `TrapContext.kernel_sp` 为子进程内核栈顶（复制来的仍是父进程的）；
6. 返回后 `sys_fork` 设子进程 `TrapContext.x[10] = 0`（用户态 `fork()` 返回 0），再 `add_task` 入队，向父进程返回子 PID。

职责边界：`fork` 只负责创建与登记，`a0=0` 与入队由 `sys_fork` 完成；父子用户数据不共享可写页。

### 1.4 程序替换：exec

`sys_exec`（`:60`）查表拿到 ELF 后调用 `TaskControlBlock::exec`（`task.rs:153`）：

- 新建 `memory_set`，更新 `trap_cx_ppn`，整体替换旧地址空间（旧 `MemorySet` 析构即回收页）；
- 用 `app_init_context` 重建完整 `TrapContext`（新入口、新用户栈、内核页表 token、**保留原内核栈顶**、`trap_handler`）；
- 保留 PID、内核栈、`task_cx`、状态、父子关系、`exit_code`、`stride`、`prio`；
- **不创建、不入队**；系统调用返回后由原有陷阱返回路径进入新程序。

参考实现只重置 `base_size`；`ch5-api` 的契约要求同时重置 `heap_bottom`、`program_brk`（见第 4 节对比）。

### 1.5 直接创建：spawn

`sys_spawn`（`:219`）→ `TaskControlBlock::spawn`（`task.rs:143`）：复用 `new` 构造完整执行环境，登记 `parent`/`children`，返回同一 `Arc`；入队与返回 PID 由 `sys_spawn` 完成，文件不存在返回 `-1`、不调用本接口。

### 1.6 调度：run_tasks / fetch / schedule

`run_tasks`（`os/src/task/processor.rs:55`）是调度循环：

```rust
loop {
    let mut processor = PROCESSOR.exclusive_access();
    if let Some(task) = fetch_task() {
        let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
        let mut task_inner = task.inner_exclusive_access();
        let next_task_cx_ptr = &task_inner.task_cx as *const TaskContext;
        task_inner.task_status = TaskStatus::Running;
        drop(task_inner);
        processor.current = Some(task);
        drop(processor);                    // 切换前释放全部动态借用
        unsafe { __switch(idle_task_cx_ptr, next_task_cx_ptr); }
    } else {
        warn!("no tasks available in run_tasks");
    }
}
```

`TaskManager::fetch`（`manager.rs:26`）实现 stride 选择：

```rust
let index = self.ready_queue.iter().enumerate()
    .min_by_key(|(_, task)| task.inner_exclusive_access().stride)?.0;
let task = self.ready_queue.remove(index)?;
let mut inner = task.inner_exclusive_access();
inner.stride += BIG_STRIDE / inner.prio;   // BIG_STRIDE = 1<<16，整数除法
```

- 以**累加前**的 `stride` 选最小；`min_by_key` 相等时返回首个，天然按队列顺序打破平局；
- 步长 `BIG_STRIDE/prio` 用 `usize` 整除，`prio > BIG_STRIDE` 时可为 0；
- 出队后状态仍为 `Ready`，`Running` 的转换与切换由 `run_tasks` 完成。

`schedule`（`processor.rs:104`）只做「保存给定现场、切回 idle」：

```rust
let mut processor = PROCESSOR.exclusive_access();
let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
drop(processor);
unsafe { __switch(switched_task_cx_ptr, idle_task_cx_ptr); }
```

它不选任务、不改 stride、不入队。`__switch`（`switch.S`）保存 `ra/sp/s0-s11`，恢复下一现场。当该进程以后被重新选中时，`schedule` 才在其内核调用流中返回。

### 1.7 主动让出与时钟抢占

`sys_yield` 与时钟中断都走 `suspend_current_and_run_next`（`task/mod.rs:39`）：

```rust
let task = take_current_task().unwrap();          // current 置 None
let mut inner = task.inner_exclusive_access();
let task_cx_ptr = &mut inner.task_cx as *mut TaskContext;
inner.task_status = TaskStatus::Ready;
drop(inner);
add_task(task);                                   // 只入队一次
schedule(task_cx_ptr);
```

保留地址空间、父子关系与调度属性；只改状态、入队、保存现场，不直接切换到别的用户进程。

### 1.8 退出、回收与孤儿移交

`exit_current_and_run_next`（`task/mod.rs:61`，由 `sys_exit` 或访存异常调用）流程：

1. `take_current_task` 取下当前进程；
2. 若 `pid == IDLE_PID(0)`：打印日志并 `panic!("All applications completed!")`（initproc 退出即整个测试结束）；
3. 否则置 `Zombie`、记 `exit_code`、**不再入队**；
4. 把它的所有 `children` 的 `parent` 改指向 `INITPROC`，push 进 `INITPROC.children`，再清空原 `children`（孤儿移交）；
5. `memory_set.recycle_data_pages()` 释放用户数据页；
6. `drop(inner)`、**`drop(task)`**；
7. 用 `TaskContext::zero_init()` 作临时现场 `schedule`——退出路径不会再恢复该现场。

退出路径必须主动 `drop(task)`：该 `Arc` 是本内核控制流持有的最后一个强引用；若不释放，父进程 `waitpid` 回收时 TCB 的强引用计数不为 1，PID、内核栈与剩余页表无法释放。这也是「不要在被放弃的退出栈上保留临时 owning 引用」的原因。

父进程侧 `sys_waitpid`（`:75`）在自己的 `children` 中查找：`pid==-1` 匹配任意子进程；不存在匹配返回 `-1`；存在匹配但无 `Zombie` 返回 `-2`；找到则 `remove(idx)`，断言强引用为 1，取 `exit_code` 写回用户内存，返回子 PID。每个子进程只能成功回收一次，失败分支不改动 `children`。

### 1.9 资源归属小结

- `PidHandle` / `KernelStack` 的 `Drop` 负责归还 PID、解除内核栈映射并归还栈编号；它们随 TCB 释放而释放。
- `MemorySet` 的页帧由 `FrameTracker` 管理，`recycle_data_pages`/整体替换/析构时回收。
- `Arc`/`Weak`：处理器与就绪队列持有强引用，父进程持有子进程强引用，子进程只弱引用父进程。
- `UPSafeCell` 是单核 `RefCell`，重复借用会 panic，因此切换前必须释放所有借用。

---

## 2. GDB 动态跟踪

以下为 `reports/ch5-gdb.log` 中的实际记录（`MODE=debug BASE=2`）。用户态帧显示为 `??`，因为内核栈上没有用户符号，用户返回现场在 `TrapContext` 中。

### 2.1 观察进程复制（fork）

断点 `os::syscall::process::sys_fork`，命中时 initproc 正在 fork 用户 shell：

```
Breakpoint 1, os::syscall::process::sys_fork () at src/syscall/process.rs:46
#0  os::syscall::process::sys_fork () at src/syscall/process.rs:46
#1  0x0000000080219b3e in os::syscall::syscall (syscall_id=220, args=...) at src/syscall/mod.rs:54
#2  0x00000000802126e4 in os::trap::trap_handler () at src/trap/mod.rs:72
#3  0x0000000000002f26 in ?? ()
pc = 0x8021ccf4   sp = 0xffffffffffffe760
```

在 `trap_cx.x[10] = 0;` 之前读取 `sys_fork` 局部量：

```
parent pid (current_task) = 0        # initproc
child  pid (new_task)     = 1
child kernel_stack id     = 1
child task_cx = TaskContext { ra: 2149656342 /* trap_return */, sp: 0xfffffffffffee000, s:[0;12] }
child trap_cx kernel_sp = 0xfffffffffffee000
child TrapContext = { x: [.. x[17]=220 ..], sstatus.bits=0x200000000,
                      sepc=11908(0x2e84), kernel_satp=.., kernel_sp=0xfffffffffffee000,
                      trap_handler=2149654706 }
child trap_cx x[10] BEFORE reset = 0
child trap_cx x[10] AFTER  reset = 0
```

解读：

- 调用链是「用户 initproc 触发 `UserEnvCall` → `trap_handler` → `syscall(id=220)` → `sys_fork` → `TaskControlBlock::fork`」；`??` 帧是用户代码，真正的用户现场在复制的 `TrapContext` 里。
- 父进程 PID 0、子进程 PID 1、子内核栈编号 1，说明 `pid_alloc`/`kstack_alloc` 各自独立分配；子 `task_cx.ra` 等于 `trap_return`、`sp` 等于子内核栈顶，验证 `goto_trap_return` 与 `trap_cx.kernel_sp` 的绑定。
- 复制来的 `TrapContext.x[17]=220` 正是 `SYS_FORK`，`x[13]/x[14]` 是调试用的 poison 值；`sepc` 是父进程 fork 返回地址。因为用户 `fork()` 传入的 `a0` 本就为 0，本次 `x[10]` 复位前后都是 0；该步骤保证「子进程 `fork()` 一定返回 0」。

### 2.2 观察调度选择（stride）

断点 `os::task::manager::TaskManager::fetch`（`manager.rs:35`，`stride` 累加前）：

```
#0  TaskManager::fetch (self=<TASK_MANAGER>) at src/task/manager.rs:35
#1  fetch_task () at src/task/manager.rs:57
#2  run_tasks () at src/task/processor.rs:58
#3  os::rust_main () at src/main.rs:113
#4  stext ()
```

连续 8 次调度的 `index / pid / stride(before) / prio`：

| 次数 | index | pid | stride(before) | prio | 累加后 |
| --- | --- | --- | --- | --- | --- |
| 1 | 0 | 0 | 0 | 16 | 4096 |
| 2 | 0 | 1 | 0 | 16 | 4096 |
| 3 | 0 | 0 | 4096 | 16 | 8192 |
| 4 | 0 | 1 | 4096 | 16 | 8192 |
| 5 | 0 | 0 | 8192 | 16 | 12288 |
| 6 | 0 | 1 | 8192 | 16 | 12288 |
| 7 | 0 | 0 | 12288 | 16 | 16384 |
| 8 | 0 | 1 | 12288 | 16 | 16384 |

解读：

- 调用链确认调度只发生在 idle 循环 `run_tasks → fetch_task → fetch`，即内核控制流经 `idle_task_cx` 选进程。
- PID 0 与 PID 1 优先级都为 16，步长 `BIG_STRIDE/prio = 65536/16 = 4096`；每次被选中只累加一次，两者交替被选，`stride` 交替增长，符合 stride 正确性。
- 前几次 `index` 均为 0：两者 `stride` 相等时选择队列中最靠前者，验证了按队列顺序打破平局。

### 2.3 程序替换（exec）

断点 `TaskControlBlock::exec`（`task.rs:171`），命中 fork 出的 PID 1 执行 `ch5b_user_shell`：

```
#0  TaskControlBlock::exec (self=...) at src/task/task.rs:171
#1  sys_exec (path=0x9030) at src/syscall/process.rs:66
#2  syscall (syscall_id=221, args=...) at src/syscall/mod.rs:55
#3  trap_handler () at src/trap/mod.rs:72
#4  0x0000000000002f56 in ?? ()
self pid = 1
entry_point = 0        # 用户 shell ELF 的 Entry 确实为 0x0
user_sp     = 86016    # 0x15000
trap_cx_ppn = PhysPageNum(536632)
```

解读：`syscall_id=221` 为 `SYS_EXEC`；进程身份（PID 1）不变，但 `memory_set`、`trap_cx_ppn` 已被新 ELF 替换，新入口 0、新用户栈顶 `0x15000`。对照 `rust-readobj`：`ch5b_user_shell.elf` 的 `Entry = 0x0`，`PT_LOAD` 段在 VA `0x0/0xA000/0xD000`，与跟踪值一致。

### 2.4 退出与回收（exit / waitpid）

运行 `ch5b_usertest` 时，先命中退出再命中回收：

```
===== exit_current_and_run_next: process becomes Zombie =====
#0  exit_current_and_run_next (exit_code=0) at src/task/mod.rs:79
#1  sys_exit (exit_code=0) at src/syscall/process.rs:29
#2  syscall (syscall_id=93, args=...) at src/syscall/mod.rs:51
#3  trap_handler () at src/trap/mod.rs:72
pid = 3, exit_code = 0

===== sys_waitpid reaps one Zombie child =====
#0  sys_waitpid (pid=12, exit_code_ptr=0x13da4) at src/syscall/process.rs:103
#1  syscall (syscall_id=260, args=...) at src/syscall/mod.rs:56
#2  trap_handler () at src/trap/mod.rs:72
pid = 12
```

解读：`syscall_id=93` 为 `SYS_EXIT`，子进程 PID 3 以退出码 0 进入 `Zombie`；`syscall_id=260` 为 `SYS_WAITPID`，父测试进程按序查找后回收了第一个已退出的子进程 PID 12，并把它从 `children` 移除。注意 `waitpid` 是按 `children` 逐个尝试的：前面仍在运行的子进程返回 `-2`，不会阻挡后面的 `Zombie` 被回收。

### 2.5 跟踪结论

- 进程创建：PID/内核栈独立分配，地址空间深拷贝，`TrapContext` 与子内核栈绑定，`a0=0` 由 `sys_fork` 负责。
- 调度：idle 循环统一经 `fetch` 选进程；按累加前 `stride` 选最小、按队列顺序打破平局、按 `BIG_STRIDE/prio` 计费。
- 程序替换：PID 不变，地址空间与 `TrapContext` 整体替换，入口/用户栈取新 ELF 值。
- 退出/回收：退出进程变 `Zombie` 且不再入队，父进程 `waitpid` 移出并最终释放其资源。

参考实现的验收输出（`make run BASE=2`）与之一致：

- `ch5b_usertest`：`Basic usertests passed!`
- `ch5_usertest`：`ch5 Usertests passed!`，其中 `ch5_stride` 的各优先级 `ratio` 近似相等。

---

## 3. 独立实现对比（`ch5-api`）

`ch5-api` 在同一数据结构与调用关系下完成 11 处接口，总体算法与参考实现一致，差异集中在接口契约、资源边界与借用/生命周期处理。

### 3.1 `exec` 的堆边界

| | 参考 `ch5` | `ch5-api` |
| --- | --- | --- |
| `base_size` | 重置为新用户栈顶 | 重置为新用户栈顶 |
| `heap_bottom` | 不修改 | **重置为新用户栈顶** |
| `program_brk` | 不修改 | **重置为新用户栈顶** |

原因：`sbrk` 的合法性以 `heap_bottom` 与 `program_brk` 为准。若替换程序后仍沿用旧程序的堆区间，新程序第一次 `sbrk(0)` 会得到旧值、堆映射也可能落在错误的逻辑段。API 契约明确要求重置，因此这里与参考实现有意不同，功能验收命令不变。

### 3.2 等待与优先级的接口拆分

- 参考实现把查找 `Zombie`、写回退出码放在 `sys_waitpid`，把改优先级放在 `sys_set_priority`，直接在 syscall 里操作 `inner`。
- `ch5-api` 把它们拆成 TCB 方法：`waitpid(&self, pid) -> Result<(usize, i32), isize>` 与 `set_priority(&self, prio) -> isize`；`waitpid` 只负责查找/移除/取退出码，**不接触用户指针**，写回由 `sys_waitpid` 完成。这样把「进程内部状态操作」与「系统调用/用户内存交互」分层，`sys_*` 只做参数转换与返回值映射。

### 3.3 调度与切换实现

`fetch`、`run_tasks`、`schedule`、`suspend_current_and_run_next`、`exit_current_and_run_next` 的实现与参考一致：使用 `min_by_key` 选最小 `stride`、`VecDeque::remove` 保持剩余顺序、切换前 `drop` 所有动态借用、退出路径主动 `drop` 局部 `Arc` 并用 `zero_init()` 现场 `schedule`。`fetch` 中相等 `stride` 依赖 `Iterator::min_by_key` 返回首个的行为来满足「队列顺序打破平局」。

### 3.4 其它

`new`/`fork`/`spawn` 的字段初始化、父子登记、`trap_cx.kernel_sp` 修正、`base_size`/堆元数据继承均与参考一致；`fork` 的 `a0=0` 与入队仍由 `sys_fork` 负责，`spawn` 的入队仍由 `sys_spawn` 负责。

### 3.5 验收

`ch5-api` 上 `make build BASE=2` 无警告；`ch5b_usertest` 输出 `Basic usertests passed!`、`ch5_usertest` 输出 `ch5 Usertests passed!`，`ch5_stride` 的优先级比例与参考一致。

---

## 4. 主要问题与解决思路

1. **GDB 名称不匹配**：NixOS 只有多架构 `gdb`，没有 `riscv64-unknown-elf-gdb`。
   解决：`os/Makefile` 增加 `GDB ?=` 并在 `debug`/`gdbclient` 使用 `$(GDB)`，本地用 `make gdbserver MODE=debug BASE=2` + `gdb`。
2. **调试内核与 release 行为差异**：进程跟踪需要更长内核栈。
   解决：`ch5` 已在各构建模式下使用 64 KiB 调试内核栈；GDB 用 `MODE=debug BASE=2`，日常验收仍用 release。
3. **切换前仍持有动态借用导致 panic**：`UPSafeCell` 是 `RefCell`。
   解决：`run_tasks` 在 `__switch` 前 `drop(task_inner)`、`drop(processor)`；`suspend`/`exit` 在 `schedule` 前释放 `inner` 与局部 `Arc`。
4. **退出进程无法被回收**：退出栈上的临时 `Arc` 会抬高强引用计数。
   解决：`exit_current_and_run_next` 主动 `drop(task)`，使父进程 `waitpid` 时强引用为 1，PID/内核栈/页表得以释放；回收路径用 `assert_eq!(Arc::strong_count(&child), 1)` 校验。
5. **`exec` 后堆行为异常（API）**：参考实现不重置堆边界。
   解决：按 API 契约在 `exec` 中同时重置 `heap_bottom`、`program_brk`。
6. **GDB 中 Arc 取值困难**：`Arc` 内部是 `ArcInner`，`print arc.pid` 失败。
   解决：切成 rust 语言后按 `(*arc.ptr.pointer).data.<field>` 读取；用户现场改从 `TrapContext`（`trap_cx->sepc/x[2]/x[10]`）读取，避免把内核 `bt` 当成用户调用栈。
7. **非零退出码判定**：`ch4b_sbrk`、`ch4_mmap1/2` 退出码为 `-2`。
   原因：这些测例本身就要求触发访存异常被内核杀死，属预期，不是缺陷。

---

## 5. 附录

- GDB 日志：`reports/ch5-gdb.log`
- 参考跟踪要点：`rcore-ch5-analyze.md`
- 独立实现：`ch5-api` 分支；接口契约见 `rcore-ch5-api.md`
