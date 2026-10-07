# rCore ch2 实验报告：用户程序、系统调用与异常处理的静态分析与 GDB 动态跟踪

本报告对应第 2 章，分析对象是 `ch2` 分支的参考实现（`make build MODE=debug BASE=2`
构建的调试内核）。报告包含静态分析与 GDB 动态跟踪两部分；独立实现与参考实现的
对比见 `ch2-api` 分支的报告。

## 0. 实验环境与构建

| 项 | 版本 / 值 |
| --- | --- |
| 宿主 | NixOS，使用仓库 `flake.nix` 提供的 dev shell（direnv 自动进入） |
| Rust | `rustc 1.80.0-nightly`，target `riscv64gc-unknown-none-elf` |
| QEMU | 7.0.0（flake 固定的 `nixpkgs-qemu7`） |
| GDB | GNU gdb 17.2，多架构，`set architecture riscv:rv64` |
| 固件 | `bootloader/rustsbi-qemu.bin` |
| 参考实现 | `ch2` 分支 |

调试构建与运行：

```bash
cd os
make build MODE=debug BASE=2                 # 带调试信息、未优化的内核
make gdbserver MODE=debug BASE=2             # 以 -s -S 启动 QEMU，等待 GDB
```

NixOS 没有 `riscv64-unknown-elf-gdb`，框架已支持 `GDB ?= riscv64-unknown-elf-gdb`，
调试时用多架构 `gdb`（`make ... GDB=gdb`）。

本机（debug 构建）关键符号地址，取自本次构建的符号表：

| 符号 | 地址 | 说明 |
| --- | --- | --- |
| `_start` / `stext` | `0x80200000` | 入口 / `.text` 起点 |
| `__alltraps` | `0x8020034c` | Trap 入口汇编 |
| `__restore` | `0x802003a8` | 恢复用户上下文并 `sret` |
| `trap_handler` | `0x8020042e` | Rust Trap 处理函数 |
| `sys_write` | `0x80201b50` | write 实现 |
| `AppManager::load_app` | `0x80202d2a` | 应用装入 |
| `run_next_app` | `0x80203008` | 批处理调度 |
| `syscall` | `0x80203d88` | 系统调用分发 |
| `sys_exit` | `0x80205bce` | exit 实现 |
| `etext` / `srodata` | `0x8020a000` | `.text` 终点 / `.rodata` 起点 |
| `erodata` / `sdata` | `0x80211000` | `.rodata` 终点 / `.data` 起点 |
| `edata` | `0x80266000` | `.data` 终点 |
| `sbss` / `boot_stack_top` | `0x80276000` | BSS 起点 / 启动栈顶 |
| `ebss` | `0x80277000` | BSS 终点 |

证据文件：

- `reports/ch2-gdb.log`：本次完整 GDB 会话日志。
- `reports/ch2-gdb.cmd`：GDB 命令脚本。
- `reports/ch2-qemu-run.log`：同一调试内核在 QEMU 中的运行输出。

## 1. 静态分析

### 1.1 构建、链接与启动

- `os/build.rs` 读取 `../user/build/bin/` 下的应用二进制，生成 `os/src/link_app.S`：
  记录应用个数 `_num_app` 与每个应用的 `app_i_start/app_i_end`，并用 `.incbin` 把
  应用镜像内联进内核 `.data` 段。因此批处理系统无需文件系统即可拿到应用。
- `os/src/linker.ld` 的 `BASE_ADDRESS=0x80200000`、`ENTRY(_start)`；`entry.asm` 设置
  启动栈后 `call rust_main`。
- `rust_main()`（[main.rs](../os/src/main.rs)）先 `clear_bss()`、初始化日志，然后
  `trap::init()` 设置 `stvec`，再 `batch::init()` 打印应用信息，最后调用
  `batch::run_next_app()` 进入第一个应用。

### 1.2 Trap 入口与初始用户态上下文

`trap::init()` 把 `stvec` 指向 `__alltraps`：

```rust
stvec::write(__alltraps as usize, TrapMode::Direct);
```

`batch::run_next_app()`（[batch.rs](../os/src/batch.rs)）取当前应用序号，
`AppManager::load_app()` 先把 `[APP_BASE_ADDRESS, +APP_SIZE_LIMIT)` 清零，再把
`app_start[app_id]` 处的应用复制到 `0x80400000`，并执行 `fence.i`。随后
`TrapContext::app_init_context(0x80400000, USER_STACK.get_sp())` 构造初始上下文：
`sstatus` 的 SPP 置为 User、`sepc = 0x80400000`、`x[2] =` 用户栈顶。最后以该上下文的
地址调用 `__restore`，由汇编恢复并 `sret` 进入 U 模式。

### 1.3 栈切换与上下文保存/恢复

[trap.S](../os/src/trap/trap.S) 的 `__alltraps`：

```asm
csrrw sp, sscratch, sp      # 交换内核栈/用户栈
addi  sp, sp, -34*8         # 在内核栈上分配 TrapContext
...                         # 保存 x1,x3..x31（跳过 x2/tp），再保存 sstatus、sepc
csrr t2, sscratch
sd   t2, 2*8(sp)            # 保存用户 sp 到 x[2]
mv   a0, sp                 # trap_handler(cx)
call trap_handler
```

`__restore` 反向操作：把 `a0` 当作 `TrapContext` 指针，恢复 `sstatus`/`sepc`/`sscratch`、
通用寄存器，释放栈帧、换回用户栈，最后 `sret`。

因此 `TrapContext`（[context.rs](../os/src/trap/context.rs)）是 `#[repr(C)]` 的
34 个机器字：`x[0..32]`、`sstatus`、`sepc`。发生 `ecall` 时保存的 `sepc` 指向这条
`ecall` 本身，`__restore` 恢复后要由处理函数负责把 `sepc` 推进到下一指令。

### 1.4 Trap 分流

[trap/mod.rs](../os/src/trap/mod.rs) 读取 `scause`/`stval` 后按 `scause.cause()` 分流：

- `UserEnvCall`：`cx.sepc += 4`，用 `cx.x[17]`（a7）和 `cx.x[10..=12]`（a0–a2）调用
  `syscall()`，把返回值写回 `cx.x[10]`，返回同一个 `cx` 交给 `__restore`。
- `StoreFault | StorePageFault`：打印 `PageFault ...` 后 `run_next_app()`。
- `IllegalInstruction`：打印 `IllegalInstruction ...` 后 `run_next_app()`。
- 其他：`panic!("Unsupported trap ...")`。

关键区别：**系统调用是可返回的**，处理完要恢复并继续该应用；**异常是终止性的**，
不再返回原上下文，而是直接切到下一个应用。

### 1.5 系统调用分发与实现

[syscall/mod.rs](../os/src/syscall/mod.rs) 的 `syscall(syscall_id, args)` 只做接口转换：

```rust
match syscall_id {
    SYSCALL_WRITE => sys_write(args[0], args[1] as *const u8, args[2]),
    SYSCALL_EXIT  => sys_exit(args[0] as i32),
    _ => panic!("Unsupported syscall_id: {}", syscall_id),
}
```

- [fs.rs](../os/src/syscall/fs.rs) 的 `sys_write(fd, buf, len)`：`fd==1` 时按 UTF-8 打印
  缓冲区并返回 `len as isize`；其他 fd 直接 `panic!`。
- [process.rs](../os/src/syscall/process.rs) 的 `sys_exit(code) -> !`：记录退出码后
  `run_next_app()`，因此不会返回当前应用。

## 2. GDB 动态跟踪

### 2.1 连接与命令

终端一运行 `make gdbserver MODE=debug BASE=2`，终端二运行 gdb 并执行
[`ch2-gdb.cmd`](ch2-gdb.cmd)，主要命令：

```gdb
set pagination off
set architecture riscv:rv64
set language c
set logging file ../reports/ch2-gdb.log
set logging enabled on
target remote localhost:1234
```

> 说明：`MODE=debug` 会经 `MAKEFLAGS` 传给子目录的 `user` 构建，用户程序也是
> debug 版本。这使 `ch2b_bad_address` 在用户态先触发 Rust 的
> `write_volatile` 前置条件 panic，而不是 StoreFault；`StoreFault/StorePageFault`
> 分支按任务书要求只做源码分析，不影响本次跟踪的系统调用与非法指令路径。

### 2.2 调用链一：`rust_main → run_next_app → load_app → __restore → 用户程序`

在 `AppManager::load_app` 首次命中断点：

```text
#0  os::batch::AppManager::load_app (app_id=0) at src/batch.rs:70
#1  os::batch::run_next_app () at src/batch.rs:138
#2  os::rust_main () at src/main.rs:92
#3  stext ()
```

`app_id = 0`，说明这是本批次的第一个应用。继续到 `__restore`：

```text
__restore: a0 = 0x8020cef0            # TrapContext 指针（内核栈上）
```

按 34 字 dump 该上下文，其中：

| 字 | 值 | 含义 |
| --- | --- | --- |
| `x[2]`（第 3 字） | `0x8020f000` | 初始用户栈顶 |
| `sstatus`（第 33 字） | `0x0000000200000000` | UXL=2（64 位），SPP=0（返回 U 模式） |
| `sepc`（第 34 字） | `0x80400000` | 应用入口 |

`__restore` 恢复这些寄存器后执行 `sret`，CPU 从 `0x80400000` 进入 U 模式。
用户栈位于内核 BSS（`USER_STACK` 静态变量），本章还没有页表隔离。

### 2.3 调用链二：异常 → `trap_handler` → `run_next_app`

等待第一条非法指令（`scause == 2`）：

```text
#0  os::trap::trap_handler (cx=0x8020cef0) at src/trap/mod.rs:41
#1  0x802003a8 in __alltraps ()

scause = 0x2      # IllegalInstruction
stval  = 0x0
saved sepc = 0x804001fc
```

`0x804001fc` 正是 `ch2b_bad_instructions` 中 `main` 的第一条指令（反汇编）：

```asm
00000000804001fc <main>:
804001fc: 73 00 20 10   sret
```

即应用在 U 模式执行了特权指令 `sret`，触发 IllegalInstruction；`stval` 为 0，
保存的 `sepc` 指向该非法指令本身。`trap_handler` 打印提示后 `run_next_app()`，
所以不会返回原上下文。`ch2b_bad_register` 的 `csrr sstatus` 同属此类。

### 2.4 调用链三：`ecall → __alltraps → trap_handler → syscall → sys_write → __restore`

等待第一条 `write`（`scause == 8 && a7 == 64`）：

```text
scause = 0x8                          # UserEnvCall
saved sepc (ecall) = 0x80402d52
syscall_id (a7)  = 64
arg a0 (fd)      = 1
arg a1 (buf ptr) = 0x8040d000
arg a2 (len)     = 37
#0 trap_handler ...
#1 __alltraps ()
```

这来自 `ch2b_hello_world`，`len=37` 对应 `"Hello, world from user mode program!\n"`。
继续进入 `syscall()`：

```text
#0 os::syscall::syscall (syscall_id=64, args=...) at src/syscall/mod.rs:25
#1 os::trap::trap_handler (...) at src/trap/mod.rs:46
#2 __alltraps ()

args = {1, 2151731200, 37}            # 2151731200 == 0x8040d000
```

`finish` 返回后，`sys_write` 的返回值是 `37`；`next` 完成写回后：

```text
sepc delta after dispatch = 0x4       # 越过 ecall
return value stored in a0 = 37        # cx.x[10]
```

用户程序里的 `ecall` 在 `0x80402d52`，其后继指令在 `0x80402d56`，与 `sepc += 4`
一致，说明恢复后从 `ecall` 的下一条指令继续执行。

### 2.5 调用链四：`sys_exit → run_next_app` 与批次结束

对 `load_app`、`sys_exit` 设置带 `commands` 的日志断点后继续，得到：

```text
[sys_exit] exit_code = 0
[load_app] app_id = 4
[sys_exit] exit_code = 0
[load_app] app_id = 5
[sys_exit] exit_code = 0
[load_app] app_id = 6
[sys_exit] exit_code = 0
[load_app] app_id = 7
```

应用正常结束后通过 `sys_exit` 回到批处理系统；`app_id=7` 表示全部 7 个应用已执行，
`load_app` 打印 `All applications completed!` 并退出 QEMU。`app_0`–`app_2` 不经过
`sys_exit`，因为它们被异常终止，直接由 `trap_handler` 调用 `run_next_app()`。

### 2.6 调用链与观察结果汇总

| 调用链 | 关键观察 |
| --- | --- |
| `rust_main → run_next_app → load_app → __restore` | `app_id=0`；`a0` 指向内核栈上的 `TrapContext`；`x[2]=0x8020f000`，`sepc=0x80400000`，`sstatus` SPP=User |
| `ecall → __alltraps → trap_handler → syscall → sys_write → __restore` | `scause=8`；`a7=64`，`a0=1`，`a1=0x8040d000`，`a2=37`；返回值 37；`sepc` 增 4 |
| 异常 → `trap_handler → run_next_app → __restore` | `scause=2`，`stval=0`，`sepc=0x804001fc`（`sret`），应用被终止 |
| `sys_exit → run_next_app` | 退出码 0，随后 `app_id=4..7`，最后 `All applications completed!` |

### 2.7 系统调用与异常的区别

- **系统调用（`scause=8`，UserEnvCall）**：应用主动请求，内核处理后可继续运行。
  必须把保存的 `sepc` 加 4（越过 `ecall`），并把返回值写入 `a0`，再返回原上下文。
- **异常（如 `scause=2`，IllegalInstruction）**：应用无法继续，内核打印提示后调用
  `run_next_app()` 装载下一个应用，不恢复原上下文、不推进 `sepc`。
- 两者都经过同一入口 `__alltraps` 和同一个 `trap_handler`，区别只在 `scause` 的分支
  以及是否返回 `cx`。

## 3. 独立实现与对比

（见 `ch2-api` 分支报告，此处从略。）

## 4. 主要问题与解决思路

（待补。）
