# rCore ch3 实验报告：任务管理的静态分析、GDB 动态跟踪与独立实现

本报告对应第 3 章。分析对象是 `ch3` 分支的参考实现（`make build MODE=debug BASE=2`
构建的调试内核）；独立实现位于 `ch3-api` 分支的
[`os/src/task/mod.rs`](../os/src/task/mod.rs)。报告包含三部分：静态分析与动态跟踪、
独立实现与对比、主要问题与解决思路。

## 0. 实验环境与构建

| 项 | 版本 / 值 |
| --- | --- |
| 宿主 | NixOS，使用仓库 `flake.nix` 提供的 dev shell（direnv 自动进入） |
| Rust | `rustc 1.80.0-nightly`，target `riscv64gc-unknown-none-elf` |
| QEMU | 7.0.0（flake 固定的 `nixpkgs-qemu7`） |
| GDB | GNU gdb 17.2，多架构，`set architecture riscv:rv64` |
| 固件 | `bootloader/rustsbi-qemu.bin` |
| 参考实现 | `ch3` 分支 |
| 独立实现 | `ch3-api` 分支 |
| 应用 | `BASE=2` 选择 13 个应用（`app_0..app_12`），见 `user/build/bin/` |

调试构建与运行：

```bash
cd os
make build MODE=debug BASE=2                 # 带调试信息、未优化的内核
make gdbserver MODE=debug BASE=2             # 以 -s -S 启动 QEMU，等待 GDB
# 另一个终端：
gdb -nx -q target/riscv64gc-unknown-none-elf/debug/os
```

NixOS 没有 `riscv64-unknown-elf-gdb`，框架已支持 `GDB ?= riscv64-unknown-elf-gdb`，
调试时用多架构 `gdb`。

本次 debug 构建的关键符号地址（取自 `rust-nm`）：

| 符号 | 地址 | 说明 |
| --- | --- | --- |
| `stext` / `_start` | `0x80200000` | 入口 / `.text` 起点 |
| `get_num_app` | `0x80201058` | 应用数量 |
| `init_app_cx` | `0x802012d8` | 生成应用首次 Trap 上下文 |
| `__switch` | `0x802020ce` | 任务上下文切换汇编 |
| `__alltraps` | `0x80202334` | Trap 入口汇编 |
| `__restore` | `0x80202390` | 恢复 Trap 上下文并 `sret` |
| `trap_handler` | `0x8020242c` | Rust Trap 处理函数 |
| `rust_main` | `0x80203874` | Rust 启动入口 |
| `TaskManager::run_first_task` | `0x80203948` | 首任务启动 |
| `TaskManager::mark_current_suspended` | `0x802039ec` | 当前任务置 `Ready` |
| `TaskManager::mark_current_exited` | `0x80203a96` | 当前任务置 `Exited` |
| `TaskManager::find_next_task` | `0x80203b40` | 轮转选择后继任务 |
| `TaskManager::run_next_task` | `0x80203c56` | 切换到后继任务 |
| `record_current_syscall` | `0x80203e32` | 系统调用计数记录 |
| `current_syscall_count` | `0x80203f32` | 查询当前任务计数 |
| `run_first_task` | `0x80204004` | task 模块对外入口 |
| `suspend_current_and_run_next` | `0x8020409c` | 对外暂停入口 |
| `exit_current_and_run_next` | `0x802040bc` | 对外退出入口 |
| `sys_write` / `set_next_trigger` | `0x8020683a` / `0x802076c0` | 写 / 设置下次时钟 |
| `sys_exit` / `sys_yield` / `sys_trace` | `0x8020778a` / `0x802078c6` / `0x80207ad8` | 相关系统调用 |
| `syscall` | `0x80207e80` | 系统调用分发 |
| `etext` / `srodata` | `0x8020c000` | `.text` 终点 / `.rodata` 起点 |
| `erodata` / `sdata` | `0x80241000` | `.rodata` 终点 / `.data` 起点 |
| `edata` / `boot_stack_lower_bound` | `0x802df000` | `.data` 终点 / 启动栈下界 |
| `boot_stack_top` / `sbss` | `0x802ef000` | 启动栈顶 / BSS 起点 |
| `ebss` | `0x8031d000` | BSS 终点 |

证据文件：

- [`reports/ch3-gdb.log`](ch3-gdb.log)：本次完整 GDB 会话日志（310 行）。
- [`reports/ch3-gdb.cmd`](ch3-gdb.cmd)：GDB 命令脚本。
- [`reports/ch3-qemu-run.log`](ch3-qemu-run.log)：参考实现 release 下的运行输出。

## 1. 静态分析

### 1.1 任务控制块与状态

[`os/src/task/task.rs`](../os/src/task/task.rs) 定义两件东西：

```rust
pub enum TaskStatus { UnInit, Ready, Running, Exited }

pub struct TaskControlBlock {
    pub task_status: TaskStatus,
    pub task_cx: TaskContext,
    pub syscall_counts: [usize; MAX_SYSCALL_NUM],
}
```

- 一个静态加载的应用对应一个任务，**任务编号就是它在 `tasks` 数组中的下标**。
- 四种状态：`UnInit`（未初始化槽位）、`Ready`（可调度）、`Running`（正在运行，含其内核
  处理过程）、`Exited`（终态）。
- `syscall_counts` 按系统调用号索引，每个任务一份，供 `sys_trace` 查询。

`TaskControlBlock` 的大小可算出：`TaskContext` 14×8 = 112 字节，`syscall_counts`
411×8 = 3288 字节，状态字段对齐占 8 字节，共 **3408 字节**。这一数值与 GDB 实测的
TCB 步长（`TCB stride = 3408`）一致，是推算任务编号的依据。

### 1.2 `TASK_MANAGER` 初始化与任务上下文

[`os/src/task/mod.rs`](../os/src/task/mod.rs) 用 `lazy_static!` 声明全局
`TASK_MANAGER`，其中 `num_app` 是实际应用数、`inner` 用 `UPSafeCell`
（`RefCell`）把可变状态延后到运行期借用检查。参考实现：

```rust
pub static ref TASK_MANAGER: TaskManager = {
    let num_app = get_num_app();
    let mut tasks = [TaskControlBlock {
        task_cx: TaskContext::zero_init(),
        task_status: TaskStatus::UnInit,
        syscall_counts: [0; MAX_SYSCALL_NUM],
    }; MAX_APP_NUM];
    for (i, task) in tasks.iter_mut().enumerate() {
        task.task_cx = TaskContext::goto_restore(init_app_cx(i));
        task.task_status = TaskStatus::Ready;
    }
    TaskManager { num_app, inner: unsafe { UPSafeCell::new(TaskManagerInner { tasks, current_task: 0 }) } }
};
```

注意参考实现的循环遍历**整个数组**（`0..MAX_APP_NUM`），而接口契约要求未使用槽位
保持 `UnInit`；这一点在 §3 对比中详述。

`init_app_cx(i)`（[`os/src/loader.rs`](../os/src/loader.rs)）在该任务的内核栈顶压入一个
`TrapContext`：`KERNEL_STACK[i].push_context(TrapContext::app_init_context(APP_BASE_ADDRESS + i*APP_SIZE_LIMIT, USER_STACK[i].get_sp()))`。
`goto_restore(kstack_ptr)` 把 `TaskContext` 的 `ra` 设为 `__restore`、`sp` 设为该
`TrapContext` 指针，于是首次被 `__switch` 恢复时先执行 `__restore`，直接进入用户态。

### 1.3 上下文切换：`TaskContext` 与 `switch.S`

```rust
#[repr(C)]
pub struct TaskContext { ra: usize, sp: usize, s: [usize; 12] }
```

[`os/src/task/switch.S`](../os/src/task/switch.S) 的 `__switch(a0=待保存, a1=待恢复)`：

```asm
sd sp, 8(a0)          # 保存 sp
sd ra, 0(a0)          # 保存 ra
.rept 12             # 保存 s0..s11 到 (n+2)*8(a0)
  SAVE_SN
.endr
ld ra, 0(a1)          # 恢复 ra
.rept 12             # 恢复 s0..s11
  LOAD_SN
.endr
ld sp, 8(a1)          # 恢复 sp
ret
```

只保存 `ra/sp/s0..s11` 这些调用约定中的被调用者保存寄存器即可：切换发生在内核态函数
调用中，其余寄存器由调用者在栈上保存。`a1` 指向新任务的 `TaskContext`，其 `ra` 要么是
`__restore`（首次运行），要么是上次切换时的返回地址（恢复运行）。

### 1.4 调度入口与调用方

| 对外接口 | 调用方 | 目的 |
| --- | --- | --- |
| `run_first_task()` | `rust_main`（[main.rs](../os/src/main.rs)） | 启动首个应用 |
| `suspend_current_and_run_next()` | `sys_yield`、时钟中断处理 | 主动让出 / 抢占 |
| `exit_current_and_run_next()` | `sys_exit`、应用异常处理 | 正常退出 / 异常终止 |

参考实现把这些逻辑封装为 `TaskManager` 的私有方法：

- `run_first_task()`：取任务 0，状态置 `Running`，`drop(inner)` 后以占位上下文
  `__switch` 进入任务 0，不再返回。
- `mark_current_suspended()` / `mark_current_exited()`：改当前任务状态。
- `find_next_task()`：在 `(current+1 ..= current+num_app)` 内取模，找第一个 `Ready`。
- `run_next_task()`：把后继置 `Running`、更新 `current_task`，`drop(inner)` 后
  `__switch`；没有 `Ready` 任务时 `panic!("All applications completed!")`。

**关键约束**：`__switch` 会换栈，所以切换前必须 `drop(inner)` 释放对 `TASK_MANAGER`
内部的 `RefMut` 借用，否则其他任务再次借用会 panic，且上下文指针的有效性也依赖借用
在切换前释放（指针指向静态内存，释放借用后仍有效）。

### 1.5 Trap 分流与系统调用计数

[`os/src/trap/mod.rs`](../os/src/trap/mod.rs) 按 `scause` 分流：

- `UserEnvCall`：`sepc += 4`，调用 `syscall(a7, [a0,a1,a2])`，把返回值写回 `a0`，返回
  同一上下文——**可返回**。
- `StoreFault/StorePageFault`、`IllegalInstruction`：打印提示后
  `exit_current_and_run_next()`——**终止性**。
- `SupervisorTimer`：`set_next_trigger()` 后 `suspend_current_and_run_next()`——**抢占**。

[`os/src/syscall/mod.rs`](../os/src/syscall/mod.rs) 在分发前先计数：

```rust
pub fn syscall(syscall_id: usize, args: [usize; 3]) -> isize {
    record_current_syscall(syscall_id);   // 先计数，包含本次调用
    match syscall_id { ... }
}
```

`record_current_syscall` / `current_syscall_count`（均在 `task/mod.rs`）通过
`TASK_MANAGER.inner` 定位 **当前任务** 的 `syscall_counts`，因此调度器必须始终维护
`current_task` 与实际运行任务一致。`sys_trace` 的请求号 `2` 返回
`current_syscall_count(id)`；由于计数在分发前完成，本次 `sys_trace` 自身也被计入。

### 1.6 时间片与状态机

[`os/src/timer.rs`](../os/src/timer.rs) 的 `set_next_trigger()` 设置
`get_time() + CLOCK_FREQ / TICKS_PER_SEC = 12500000/100 = 125000` 个周期后触发，
即 **10 ms** 一次时钟中断。每次中断把当前任务从 `Running` 变为 `Ready` 并轮转，形成
时间片轮转：

```
UnInit --init--> Ready --schedule--> Running --yield/timer--> Ready
                                        |--exit/exception--> Exited(终态)
```

## 2. GDB 动态跟踪

### 2.1 连接与命令

终端一运行 `make gdbserver MODE=debug BASE=2`，终端二按
[`ch3-gdb.cmd`](ch3-gdb.cmd) 执行，核心命令：

```gdb
set pagination off
set architecture riscv:rv64
set language c
set logging file ../reports/ch3-gdb.log
set logging enabled on
target remote localhost:1234
```

任务编号由上下文地址反推：`index = (cx_addr - task0_cx_addr) / 3408`。

### 2.2 首个任务启动：`run_first_task → TASK_MANAGER 初始化 → __switch → __restore`

在 `os::task::run_first_task` 命中（`TASK_MANAGER` 尚未首次解引用）：

```text
#0  os::task::run_first_task () at src/task/mod.rs:164
#1  0x00000000802038bc in os::rust_main () at src/main.rs:105
#2  0x0000000080200010 in stext ()

-- get_num_app returned 13
-- init_app_cx called 16 times          # 参考实现遍历整个数组
```

`get_num_app()` 返回 **13**（`BASE=2` 的应用数），但 `init_app_cx` 被调用 **16** 次
（`MAX_APP_NUM`），证实参考实现把整个任务数组都初始化了。

随后在第一个 `__switch` 入口：

```text
a0 = 0x802eeed8     # 待保存：启动阶段的临时上下文（启动栈）
a1 = 0x8030f130     # 待恢复：任务 0 的 TaskContext
ra = 0x802039be     # 调用点（TaskManager::run_first_task 内）
sp = 0x802eeea0

x/14gx 0x8030f130:
0x8030f130: 0x0000000080202390 0x000000008020eef0   # ra=__restore, sp=TrapContext
0x8030f140: 0x0000000000000000 0x0000000000000000   # s0..s11 全 0
...
```

对照 `TaskContext` 布局：第 0 字 `ra = 0x80202390 = __restore`，第 1 字
`sp = 0x8020eef0`（内核栈上的 `TrapContext`），第 2–13 字为 `s0..s11`（`goto_restore`
初始化为 0）。这说明首次恢复会先跳到 `__restore`。

继续到 `__restore`：

```text
sp = 0x8020eef0 <os::loader::KERNEL_STACK+7920>     # 指向 TrapContext（34 字）
x/34gx 0x8020eef0:
  x[2]   = 0x8022e000     # 初始用户栈顶
  sstatus= 0x0000000200000000   # UXL=2（64 位）、SPP=0（返回 U 模式）
  sepc   = 0x80400000     # 应用入口（task 0）
-- sepc = 0x80400000 ; user sp x[2] = 0x8022e000
```

`__restore` 恢复 `sstatus/sepc/sscratch` 与通用寄存器后 `sret`，CPU 从 `0x80400000`
进入 U 模式。任务 0 的内核栈偏移 `+7920` 位于第 0 个槽位（`7920/8192 = 0`），与任务
编号一致。

### 2.3 时钟抢占：`时钟中断 → set_next_trigger → suspend → __switch`

在 `trap_handler` 以 `$scause == 0x8000000000000005` 命中：

```text
#0  os::trap::trap_handler (cx=0x8020eef0 <KERNEL_STACK+7920>) at src/trap/mod.rs:49
#1  0x0000000080202390 in __alltraps ()
scause = 0x8000000000000005     # SupervisorTimer
sepc   = 0x80400000             # 应用 0 尚未执行第一条指令即被抢占
```

调用链与寄存器：

```text
set_next_trigger          # trap/mod.rs:68
suspend_current_and_run_next   # trap/mod.rs:69  (mark_current_suspended -> run_next_task)
TaskManager::find_next_task
__switch:
  a0 = 0x8030f130   # 任务 0 的 TaskContext（当前，待保存）
  a1 = 0x8030fe80   # 任务 1 的 TaskContext（后继，待恢复）
  ra = 0x80203e14   # run_next_task 中 __switch 的返回点
-- TCB stride = 3408
-- current task index = 0, successor index = 1
```

任务 1 的上下文 dump 显示 `ra = 0x80202390 (__restore)`、`sp = 0x80210ef0`，说明它也是
首次运行。这印证了抢占路径：时钟中断把任务 0 由 `Running` 改为 `Ready`，轮转选中任务
1（`num_app = 13` 范围内），并更新 `current_task` 后切换。

### 2.4 系统调用计数

在 `record_current_syscall` 首次命中断点：

```text
#0  os::task::record_current_syscall (syscall_id=64) at src/task/mod.rs:144
#1  0x0000000080207e9c in os::syscall::syscall (syscall_id=64, args=...) at src/syscall/mod.rs:34
#2  0x000000008020293e in os::trap::trap_handler (cx=0x80216ef0 <KERNEL_STACK+40688>) at src/trap/mod.rs:57
#3  0x0000000080202390 in __alltraps ()
-- syscall_id = 64
```

`0x80216ef0 = KERNEL_STACK + 40688`，`40688/8192 = 4.97`，即任务 **4**；`syscall_id=64`
是 `write`。调用栈说明记录发生在 `syscall()` 分发之前，且计数归属 `current_task`
（此处任务 4）的 `syscall_counts`。

> 调试构建的局限：`MODE=debug` 会把 `user` 子构建也编成 debug，导致 `ch3_sleep` 在
> `get_time()==0` 上断言失败、`ch3_trace` 无法完成，因此本次 debug 会话没有命中
> `sys_trace`（`id=410`）。`sys_trace` 的查询路径按 §1.5 源码分析：它调用
> `current_syscall_count(id)`，后者用 `inner.current_task` 定位当前任务再读取
> `syscall_counts`，因此每个任务的计数互不影响。release 运行
> （[`ch3-qemu-run.log`](ch3-qemu-run.log)）能正常通过 `Test trace OK!`。

### 2.5 `sys_yield`：暂停与恢复

在 `sys_yield` 命中：

```text
#0  os::syscall::process::sys_yield () at src/syscall/process.rs:23
#1  0x0000000080207f84 in os::syscall::syscall (syscall_id=124, args=...)
#2  0x000000008020293e in os::trap::trap_handler (cx=0x80220ef0 <KERNEL_STACK+81648>)
#3  0x0000000080202390 in __alltraps ()
```

`0x80220ef0 = KERNEL_STACK + 81648`，`81648/8192 = 9.97`，即任务 **9**。继续进入
`suspend_current_and_run_next`（`mark_current_suspended → run_next_task`）与
`find_next_task`，最后在 `__switch` 入口：

```text
a0 = 0x80316900   # 任务 9 的 TaskContext（待保存）
a1 = 0x80317650   # 任务 10 的 TaskContext（待恢复）
ra = 0x80203e14
sp = 0x80220740   # KERNEL_STACK+79680
-- current task index = 9, successor index = 10

-- context to save (0x80316900):
   0x80202390 0x80220ef0 ...      # 仍为首次运行上下文（本次 save 尚未执行）
-- context to restore (0x80317650):
   0x80202390 0x80222ef0 ...      # 任务 10 首次运行
-- suspended task will resume at ra=0x80203e14 with sp=0x80220740
```

随后在保存的返回地址 `0x80203e14` 且 `sp == 0x80220740` 的条件断点处命中：

```text
Temporary breakpoint 17, 0x0000000080203e14 in os::task::TaskManager::run_next_task (...)
#0  run_next_task  src/task/mod.rs:133
#1  run_next_task  src/task/mod.rs:170
#2  suspend_current_and_run_next  src/task/mod.rs:186
#3  sys_yield  src/syscall/process.rs:24
#4  syscall  src/syscall/mod.rs:38
#5  trap_handler  src/trap/mod.rs:57
ra = 0x80203e14   sp = 0x80220740
```

这直接证明：被暂停的任务 9 之后在**同一个返回点、同一个内核栈**上恢复，`sys_yield`
按调用栈逐层返回，系统调用与 Trap 处理得以继续。

### 2.6 任务退出：`sys_exit → exit_current_and_run_next → 后继任务`

```text
#0  os::task::exit_current_and_run_next () at src/task/mod.rs:191
#1  os::syscall::process::sys_exit (exit_code=-1) at src/syscall/process.rs:17
#2  os::syscall::syscall (syscall_id=93, ...) at src/syscall/mod.rs:37
#3  os::trap::trap_handler (cx=0x80214ef0 <KERNEL_STACK+32496>) at src/trap/mod.rs:57
#4  __alltraps ()

-- mark_current_exited
-- find_next_task after exit (Exited slot excluded)
__switch:
  a0 = 0x80311920    # 任务 3（已 Exited，待保存）
  a1 = 0x80312670    # 任务 4（待恢复）
-- current task index = 3, successor index = 4
```

任务 3（`KERNEL_STACK+32496`，`32496/8192 ≈ 3.97`）通过 `sys_exit` 退出，被置为
`Exited`；`find_next_task` 只匹配 `Ready`，因此跳过它选中任务 4。后继任务 4 的上下文
dump 出现 `ra = 0x80203e14`（而不是 `__restore`），说明它此前已被抢占过，保存的是
可恢复的内核返回点：

```text
0x80312670: 0x0000000080203e14 0x0000000080216880   # ra=run_next_task, sp=内核栈
0x80312680: 0x0000000080216970 0x0000000000000000   # s0, ...
```

这正好对比了「首次运行（`ra=__restore`）」与「再次恢复（`ra=run_next_task`）」两种
上下文。

### 2.7 观察结果汇总

| 调用链 | 关键观察 |
| --- | --- |
| `rust_main → run_first_task → TASK_MANAGER 初始化 → __switch → __restore` | `num_app=13`，`init_app_cx` 调用 **16** 次；任务 0 上下文 `ra=__restore`、`sp=TrapContext`、`sepc=0x80400000` |
| 时钟中断 → `set_next_trigger → suspend_current_and_run_next → __switch` | `scause=0x8000000000000005`；抢占时 `sepc=0x80400000`；切换 `0→1`，TCB 步长 3408 |
| `syscall → record_current_syscall` | `syscall_id=64`（write）在分发前计数，归属 `current_task` |
| `sys_yield → suspend → find_next → __switch → 恢复` | 任务 9 让出，切换 `9→10`；暂停上下文 `ra=0x80203e14, sp=0x80220740`，并在同一 `ra/sp` 恢复 |
| `sys_exit → exit_current_and_run_next → 后继` | 任务 3 退出（`Exited`），切换 `3→4`；后继上下文 `ra=run_next_task`（可恢复） |
| 无就绪任务 | 参考实现在 `task/mod.rs:137` `panic!("All applications completed!")` |

## 3. 独立实现与对比

独立实现只修改一个文件、完成四处 TODO：
[`os/src/task/mod.rs`](../os/src/task/mod.rs) 的 `TASK_MANAGER` 初始化表达式、
`run_first_task()`、`suspend_current_and_run_next()`、`exit_current_and_run_next()`；
`os/src/task/task.rs` 无需改动，`entry.asm` 的 64 KiB 启动栈经 release / debug 实测
均未溢出，故未修改。

实现组织：在 `impl TaskManager` 中新增私有方法 `run_first_task`、
`mark_current_suspended`、`mark_current_exited`、`find_next_task`、`run_next_task`，
三个对外函数只做薄封装。

### 3.1 初始化范围（最重要的差异）

- **参考实现**：`for (i, task) in tasks.iter_mut().enumerate()` 遍历 `0..MAX_APP_NUM`
  （16），把**所有槽位**都置为 `Ready` 并调用 `init_app_cx(i)`。GDB 实测
  `init_app_cx` 被调用 **16** 次，而实际应用只有 13 个，即多初始化了 3 个无效槽位。
  由于 `find_next_task` 只在 `0..num_app` 内轮转，这些多余槽位不会被调度，行为上无害。
- **本次实现**：按接口契约只初始化 `0..num_app`：

  ```rust
  let mut tasks = [TaskControlBlock {
      task_status: TaskStatus::UnInit,
      task_cx: TaskContext::zero_init(),
      syscall_counts: [0; MAX_SYSCALL_NUM],
  }; MAX_APP_NUM];
  for (i, task) in tasks.iter_mut().enumerate().take(num_app) {
      task.task_cx = TaskContext::goto_restore(init_app_cx(i));
      task.task_status = TaskStatus::Ready;
  }
  ```

  未使用槽位保持 `UnInit`，满足契约，也避免为不存在的应用生成上下文。

### 3.2 轮转选择

两者都在有效编号 `0..num_app` 内、从当前任务之后开始取模寻找第一个 `Ready`：

```rust
(cur + 1 ..= cur + self.num_app).map(|id| id % self.num_app)
    .find(|id| inner.tasks[*id].task_status == TaskStatus::Ready)
```

`UnInit` 与 `Exited` 天然不会被选中。`suspend` 时当前任务已改回 `Ready`，若只有它一个
就选回自身；`exit` 时当前任务已 `Exited`，不会自选。与 GDB 观察到的 `0→1`、`9→10`、
`3→4` 轮转一致。

### 3.3 状态变化与恢复

状态迁移与参考一致：`suspend` 把 `Running→Ready` 并保留可恢复上下文；`exit` 把
`Running→Exited`（终态）并让后继继续。恢复时 `__switch` 载入 `ra/sp/s0..s11`，被暂停
任务的调用栈得以延续（§2.5 的条件断点即证明）。

### 3.4 借用与上下文指针

两者都在 `__switch` 前 `drop(inner)`。本实现额外在 `run_first_task` 中显式写入
`current_task = 0`，并在注释中说明上下文指针指向静态 `TASK_MANAGER`、释放借用后仍
有效。这样既避免 `RefCell` 的运行期借用冲突，也保证切换期间指针有效。

### 3.5 内部组织

参考实现把逻辑分散在 `impl TaskManager` 的多个方法与同名的模块级包装函数中；本实现
采用同样的分层（方法 + 薄封装），但由我们自行命名与拆分，接口签名、返回类型与
可见性保持不变。差异仅在内部组织，不影响行为。

### 3.6 验收

`ch3-api` 分支上执行 `cd os && make run CHAPTER=3 BASE=2`，通过全部断言：

```text
Test write A OK!
Test write B OK!
Test write C OK!
Test sleep OK!
Test sleep1 passed!
Test trace OK!
```

## 4. 主要问题与解决思路

### 4.1 NixOS 没有 `riscv64-unknown-elf-gdb`

- **现象**：按任务书直接使用 `riscv64-unknown-elf-gdb` 会找不到命令。
- **解决**：`os/Makefile` 已把调试器抽成变量 `GDB ?= riscv64-unknown-elf-gdb`，本地用
  多架构 `gdb`（17.2）。相应地在每个实验分支做一次 Makefile 适配（步骤记录在
  `main` 分支的 `docs/nixos-branch-adaptation.md`）。

### 4.2 `ch3-api` 的 `CHAPTER` 自动探测

- **现象**：在 `ch3-api` 上直接 `make run` 会得到 `CHAPTER=3-api`，报
  `undefined symbol: app_18446744073709551615_end`。
- **原因**：Makefile 里 `sed -E 's/ch([0-9])/\1/'` 会保留 `-api` 后缀。
- **解决**：改为与 `ch2-api` 一致的 `sed -E 's/^ch([0-9]+).*$$/\1/'`，`chN`/`chN-api`
  都解析为 `N`；验收命令也始终显式传 `CHAPTER=3`。

### 4.3 debug 用户构建导致 `ch3_sleep` 断言失败

- **现象**：`make run MODE=debug BASE=2` 下 `ch3_sleep` 在
  `src/bin/ch3_sleep.rs:16` 报 `assertion failed: current_time > 0`，`ch3_trace` 也无法
  完成，因此 GDB 会话看不到 `sys_trace`。
- **原因**：`MODE` 经 `MAKEFLAGS` 传播到 `user` 子构建，用户程序也被编成 debug 版，
  运行行为与 release 不同（与 ch2 报告中 `ch2b_bad_address` 的现象同类）。
- **处理**：`sys_trace` 的查询路径以源码分析说明（§1.5、§2.4），功能验收使用 release
  运行 [`ch3-qemu-run.log`](ch3-qemu-run.log)，`Test trace OK!`、`Test sleep OK!` 均通过。
  该限制与本章功能实现无关。

### 4.4 GDB 对 Rust 函数的断点行号

- **现象**：`tbreak os::syscall::process::sys_trace` 被落在 `process.rs:43`（`trace!` 之后
  的某个分支）而非函数入口；`current_syscall_count` 被内联进 `sys_trace`，没有独立调用，
  因此对它的断点不会命中。
- **解决**：分析 `sys_trace` 时直接使用函数入口地址 `0x80207ad8`；判断「是否内联」时用
  `rust-objdump` 查找对目标符号的调用。最终该路径以源码分析为主（§2.4）。

### 4.5 后台启动 QEMU 与 `cd`

- **现象**：把 `cd os && qemu ... &` 写成一条命令时，`cd` 只作用于被后台化的子 shell，
  随后的 gdb 在仓库根目录运行，报 `No such file or directory`。
- **解决**：先 `cd` 再单独后台启动 QEMU；或对 gdb 使用绝对路径。另注意不要用
  `pkill -f 'qemu-system-riscv64'`，该模式会匹配当前命令自身而误杀 shell（ch2 报告已
  记录同类问题）。

### 4.6 启动栈

- **现象**：接口文档提示 `TASK_MANAGER`（约 52 KiB）在 debug 下可能撑爆 64 KiB 的
  `boot_stack`。
- **验证**：本次 debug 构建与运行、GDB 会话均正常，未出现栈溢出，故未修改
  `entry.asm`。若后续应用更多或编译器临时副本增大，可按契约把 `.space 4096 * 16`
  调大并说明。

### 4.7 尚未解决 / 说明

- debug 下 `ch3_sleep`/`ch3_trace` 的行为差异源自用户程序 debug 构建，本次通过 release
  运行补足验收证据；如需在 debug 下复现 trace，可单独以 release 构建 `user` 后只调试
  内核，但这超出本章要求。
- 无就绪任务时的「全部完成」在参考实现里是 `panic!`，本实现保持一致；更优雅的关机
  路径不在本章契约内。
