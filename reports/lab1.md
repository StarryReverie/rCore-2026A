# rCore ch1 实验报告：裸机 Rust 入口的静态分析、GDB 动态跟踪与独立实现

本报告对应第 1 章。分析对象是 `ch1` 分支的参考实现（提交 `476dce5`），独立实现位于 `ch1-api` 分支的 [`os/src/main.rs`](../os/src/main.rs)。报告包含三部分：静态分析与动态跟踪、独立实现与对比、主要问题与解决思路。

## 0. 实验环境与构建

| 项 | 版本 / 值 |
| --- | --- |
| 宿主 | NixOS，使用仓库 `flake.nix` 提供的 dev shell（direnv 自动进入） |
| Rust | `rustc 1.80.0-nightly (c987ad527 2024-05-01)`，target `riscv64gc-unknown-none-elf` |
| QEMU | 7.0.0（flake 固定的 `nixpkgs-qemu7`） |
| GDB | GNU gdb 17.2，`--enable-targets=all`，可直接 `set architecture riscv:rv64` |
| 固件 | `bootloader/rustsbi-qemu.bin`（RustSBI 0.3.0-alpha.4 / RustSBI-QEMU 0.2.0-alpha.2） |
| 参考实现 | `ch1` 分支，提交 `476dce5` |
| 独立实现 | `ch1-api` 分支 |

参考实现的调试构建（用于 GDB 跟踪）：

```bash
cd os
LOG=TRACE cargo build          # 等价于 make build MODE=debug LOG=TRACE
rust-objcopy target/riscv64gc-unknown-none-elf/debug/os --strip-all -O binary \
  target/riscv64gc-unknown-none-elf/debug/os.bin
```

`MODE=debug` 生成带调试信息、未优化的内核；`LOG` 由 `option_env!("LOG")` 在编译期读取（见 `os/src/logging.rs`），因此必须在构建命令中传入，运行后再设置环境变量无效。

本机（debug 构建）关键符号地址，均取自本次构建的符号表：

| 符号 | 地址 | 说明 |
| --- | --- | --- |
| `_start` / `stext` | `0x80200000` | 入口 / `.text` 起点 |
| `etext` / `srodata` | `0x80206000` | `.text` 终点 / `.rodata` 起点 |
| `erodata` / `sdata` | `0x80208000` | `.rodata` 终点 / `.data` 起点 |
| `edata` / `boot_stack_lower_bound` | `0x80209000` | `.data` 终点 / 启动栈下界 |
| `boot_stack_top` / `sbss` | `0x80219000` | 启动栈顶 / BSS 起点 |
| `ebss` / `ekernel` | `0x8021a000` | BSS 终点 / 内核终点 |
| `clear_bss` 闭包 | `0x802029c0` | `for_each` 闭包入口 |
| 闭包中的 `sb` | `0x802029fa` | 真正写零的指令 |
| `console::print` | `0x80201c84` | 控制台打印入口 |
| `console_putchar` | `0x80201d8c` | 单字符输出入口 |
| `console_putchar` 中的 `ecall` | `0x80201db0` | SBI 调用 |
| `exit` | `0x80202658` | QEMU 退出实现 |
| `exit_success` | `0x802026d8` | 成功退出包装 |
| `exit` 中的 MMIO `sw` | `0x802026ce` | 写退出设备 |

证据文件：

- `reports/ch1-gdb.log`：参考实现的完整 GDB 会话日志（234 行）。
- `reports/ch1-api-run.log`：独立实现的运行输出。

## 1. 静态分析

### 1.1 构建与加载

- `os/Makefile`：`TARGET=riscv64gc-unknown-none-elf`，`KERNEL_ENTRY_PA=0x80200000`；运行命令用 `-bios ../bootloader/rustsbi-qemu.bin -device loader,file=$(KERNEL_BIN),addr=0x80200000` 把内核二进制加载到 `0x80200000`。
- `os/.cargo/config.toml`：`-Clink-arg=-Tsrc/linker.ld -Cforce-frame-pointers=yes`，链接脚本显式指定，栈回溯因此可用。
- `os/src/main.rs`：`#![no_std]` 表示不依赖宿主标准库；`#![no_main]` 表示不使用 Rust 运行时安排的 `main` 入口；`global_asm!(include_str!("entry.asm"))` 把入口汇编并入内核；`#[no_mangle]` 保留汇编 `call` 所需的符号名。

### 1.2 链接脚本与入口

`os/src/linker.ld` 中 `BASE_ADDRESS=0x80200000`、`ENTRY(_start)`，`.text` 段先放 `*(.text.entry)`，因此 `entry.asm` 的 `_start` 落在 `0x80200000`，与 `stext` 重合。`entry.asm`：

```asm
.section .text.entry
.globl _start
_start:
    la sp, boot_stack_top      # 设置栈指针
    call rust_main

.section .bss.stack
.globl boot_stack_lower_bound
boot_stack_lower_bound:
    .space 4096 * 16           # 64 KiB 启动栈
.globl boot_stack_top
boot_stack_top:
```

进入 Rust 前栈已可用。链接脚本在 `.bss` 节内先放 `*(.bss.stack)` 再定义 `sbss`，所以 `[sbss, ebss)` 不包含启动栈，`clear_bss()` 不会清掉正在使用的栈。

### 1.3 `rust_main` 与初始化顺序

参考实现（`ch1:os/src/main.rs`）的核心：

```rust
clear_bss();
logging::init();
println!("[kernel] Hello, world!");
trace!(/* .text   */);
debug!(/* .rodata */);
info!( /* .data   */);
warn!( /* boot_stack */);
error!(/* .bss    */);
crate::board::QEMU_EXIT_HANDLE.exit_success();
```

顺序原因：`clear_bss()` 用逐字节 volatile 写把 `[sbss, ebss)` 清零。裸机环境没有宿主启动逻辑替内核清零全局存储，而 `logging::init()` 会写全局状态（`log::set_logger`、`set_max_level`），这些状态位于 BSS，所以必须先清零再初始化日志；反之若在日志初始化后再清零，会把已建立的日志状态覆盖掉。

### 1.4 输出路径

`println!` 来自 `os/src/console.rs`，其 `print` 调用 `Stdout.write_fmt(format_args!(...))`；`Stdout::write_str` 对每个字符调用 `console_putchar`；`console_putchar` 调用 `#[inline(always)]` 的 `sbi_call(SBI_CONSOLE_PUTCHAR, c, 0, 0)`，最终执行 `ecall`。这是一条纯 SBI 路径，不依赖宿主标准输出。

### 1.5 退出路径

`crate::board::QEMU_EXIT_HANDLE.exit_success()` 来自 `os/src/boards/qemu.rs`，它调用 `exit(EXIT_SUCCESS)`，向 QEMU `virt` 的 `sifive_test` 设备（`VIRT_TEST=0x100000`）写入 `0x5555`（成功退出编码）。`exit` 返回 `!`，写入后进入 `wfi` 死循环；QEMU 收到该 MMIO 写后退出。注意本章正常退出走板级 MMIO，不经过 `sbi::shutdown()`——后者当前调用的是 `exit_failure()`，只用于 panic 路径。

## 2. GDB 动态跟踪

### 2.1 连接与命令

两个终端：一个用 `make gdbserver MODE=debug LOG=TRACE`（以 `-s -S` 启动等待调试的 QEMU），另一个用 GDB 读取 debug 版 ELF 并连接 `localhost:1234`。本次使用的 GDB 命令（节选）：

```gdb
set pagination off
set logging file ../reports/ch1-gdb.log
set logging enabled on
set architecture riscv:rv64
set language c
target remote localhost:1234
```

随后按执行顺序用临时断点 `tbreak` 与 `continue` 推进；用 `bt`、`info registers`、`x/i`、`disassemble /r`、`si` 观察状态。连接后首先停在 `0x1000`（固件阶段），这是正常的。

### 2.2 入口与启动栈

```gdb
tbreak _start
continue
```

命中 `stext+12`（`jalr -1366(ra)`，即 `call rust_main`）前，观察：

```text
pc = 0x8020000c <stext+12>
sp = 0x80219000 <log::STATE>      # 已等于 boot_stack_top
$1 = 0x80209000                   # boot_stack_lower_bound
$2 = 0x80219000                   # boot_stack_top
$3 = 65536                        # 栈大小 = 64 KiB
```

`la sp, boot_stack_top` 是伪指令，展开为 `auipc` + `mv`；执行后 `sp` 等于栈顶。进入 `rust_main` 时：

```text
Temporary breakpoint 2, os::rust_main () at src/main.rs:54
#0  os::rust_main () at src/main.rs:54
#1  0x0000000080200010 in stext ()
pc = 0x80200ac6 <os::rust_main+20>
sp = 0x80218230                   # 函数序言已分配栈帧，sp 降低但仍在启动栈内
```

`rust_main` 的序言（`addi sp,sp,-0x7f0` 等）把 `sp` 从 `0x80219000` 降到 `0x80218230`，仍位于 `[0x80209000, 0x80219000)` 内，说明栈使用合法。

### 2.3 BSS 清零

```gdb
tbreak os::clear_bss
continue
set $bss_begin = (unsigned long)&sbss
set $bss_end   = (unsigned long)&ebss
p/d $bss_end - $bss_begin
```

得到 `[sbss, ebss) = [0x80219000, 0x8021a000)`，共 4096 字节。调用栈印证了 `Range::for_each` 与闭包的关系：

```text
#0  os::clear_bss () at src/main.rs:36
#1  ... os::rust_main () at src/main.rs:54
```

进入闭包后，在真正的写指令处 `si` 单步：

```text
Temporary breakpoint 5, 0x802029fa in os::clear_bss::{closure#0}
pc = 0x802029fa
a0 = 0x0
a1 = 0x80219000
=> 0x802029fa: sb a0,0(a1)        # 向 sbss 首字节写 0
```

`a1=sbss`、`a0=0`，证明正在把区间首地址写零。执行 `si` 后 `x/8bx $bss_begin` 仍显示全零：**清零前后内存都为 0，不能仅凭零值证明清零发生过**，本报告以命中的 `sb` 写指令、其访问地址 `sbss` 以及"先 `clear_bss` 后 `logging::init`"的调用顺序作为证据。闭包按左闭右开区间迭代，末地址 `ebss` 不参与写入。

随后命中 `os::logging::init`（`src/logging.rs:36`），调用栈为 `logging::init ← rust_main ← stext`，说明清零调用已返回，才开始建立日志全局状态，顺序与源码一致。

### 2.4 字符输出与 `ecall`

```gdb
tbreak os::console::print
continue
```

```text
#0  os::console::print (args=...) at src/console.rs:17
#1  0x0000000080200b1e in os::rust_main () at src/console.rs:32
```

继续捕获第一个字符：

```text
Temporary breakpoint 8, os::sbi::console_putchar (c=91) at src/sbi.rs:26
$7 = 0x5b                         # 91 = '['，即 "[kernel] ..." 的首字符
#0  os::sbi::console_putchar (c=91) at src/sbi.rs:26
#1  os::console::{impl#0}::write_str (self=0x802181e7, s=...) at src/console.rs:10
#2  core::fmt::write () at library/core/src/fmt/mod.rs:1182
#3  core::fmt::Write::write_fmt::{impl#1}::spec_write_fmt<os::console::Stdout> ...
#4  core::fmt::Write::write_fmt<os::console::Stdout> ...
#5  os::console::print (args=...) at src/console.rs:17
#6  os::rust_main () at src/console.rs:32
```

`console_putchar` 中 `sbi_call` 被内联，反汇编可看到 `ecall` 在 `0x80201db0`。在该地址设临时断点并继续：

```text
=> 0x80201db0 <...console_putchar+36>: ecall
a0 = 0x5b     # 字符参数 c
a1 = 0x0
a2 = 0x0
a6 = 0x0
a7 = 0x1      # SBI 调用号 SBI_CONSOLE_PUTCHAR = 1
```

`a7=1` 是 SBI 的 `console_putchar` 调用号，`a0` 是要输出的字符 `'['`，与 `os/src/sbi.rs` 中的约定一致。

### 2.5 成功退出

```gdb
tbreak *0x802026d8      # exit_success
continue
disassemble /r 0x80202658,0x802026d8
tbreak *0x802026ce      # exit 内的 MMIO sw
continue
```

在 MMIO 指令处：

```text
#0  0x802026ce in os::board::{impl#1}::exit (self=0x80206428, code=21845)
#1  0x802026f2 in os::board::{impl#1}::exit_success (self=0x80206428)
$8  = 0x5555        # code = EXIT_SUCCESS
$9  = 0x80206428    # self
$10 = 0x100000      # self->addr = VIRT_TEST
a0  = 0x5555
a1  = 0x100000
=> 0x802026ce: sw a0,0(a1)
```

`a0=0x5555`、`a1=0x100000`，即向 `sifive_test` 设备寄存器写入成功退出码。执行 `si` 后 GDB 报告 `Remote connection closed`，QEMU 进程随即结束——这正是正常退出的表现。严格地说，单独看到连接断开不能证明运行成功，必须结合此前的 `Hello, world!`、五条布局日志与终端退出码 `0` 才能构成完整的成功证据（见 `reports/ch1-api-run.log`）。

## 3. 调用链总结

| 调用链 | 本次观察到的证据 |
| --- | --- |
| QEMU/RustSBI → `_start` → 设 `sp` → `rust_main` | 连接后停在固件 `0x1000`；`_start`/`stext=0x80200000`；`sp=0x80219000`（栈顶），栈 `[0x80209000,0x80219000)` 长 65536；进入 `rust_main` 后 `sp=0x80218230` |
| `rust_main` → `clear_bss` → `Range::for_each` → 闭包 → `write_volatile` | `[sbss,ebss)=0x80219000..0x8021a000`；闭包中 `sb a0,0(a1)`，`a0=0`、`a1=sbss`；随后才命中 `logging::init` |
| `println!` → `console::print` → 格式化 → `Stdout::write_str` → `console_putchar` → 内联 `sbi_call` → `ecall` | 首个字符 `c=91('[')`；调用栈经 `write_str`、`core::fmt::write`、`write_fmt`；`ecall` 前 `a0=0x5b, a7=1` |
| `rust_main` → `exit_success` → `exit` → MMIO `sw` → QEMU 退出 | `code=0x5555`、`self->addr=0x100000`；`sw a0,0(a1)` 前 `a0=0x5555,a1=0x100000`；执行后 QEMU 退出，退出码 0 |

## 4. 独立实现

### 4.1 接口契约

`os/src/main.rs` 的 `rust_main() -> !` 要求：先 `clear_bss()` 再 `logging::init()`；打印 `[kernel] Hello, world!`；对局部 `usize` 数组 `[1,2,3,4,5]` 实际遍历求和并打印 `[kernel] sum = 15`；按原有宏、级别、格式与顺序输出五条内存布局日志；最后用 `QEMU_EXIT_HANDLE.exit_success()` 成功退出。仍需满足：保留入口属性与链接符号声明、不使用 `std`/堆分配/用户态系统调用、不修改 `clear_bss()` 与其他文件。

### 4.2 实现代码

只替换了骨架中的 `todo!`，未改动同文件其他部分：

```rust
#[no_mangle]
#[allow(dead_code, unused_imports)]
pub fn rust_main() -> ! {
    use crate::board::QEMUExit;

    extern "C" {
        fn stext();
        fn etext();
        fn srodata();
        fn erodata();
        fn sdata();
        fn edata();
        fn sbss();
        fn ebss();
        fn boot_stack_lower_bound();
        fn boot_stack_top();
    }

    // 1. 未初始化的全局存储先清零，之后才能安全地建立日志等全局状态。
    clear_bss();
    logging::init();

    // 2. 输出启动信息（由提供的 SBI 控制台，不依赖宿主标准输出）。
    println!("[kernel] Hello, world!");

    // 3. 在启动栈上定义局部数组，实际遍历元素求和并输出计算结果。
    let numbers: [usize; 5] = [1, 2, 3, 4, 5];
    let mut sum: usize = 0;
    for number in numbers.iter() {
        sum += *number;
    }
    println!("[kernel] sum = {}", sum);

    // 4. 按原有日志宏、级别、格式与顺序记录内存布局，地址取自链接符号。
    trace!("[kernel] .text [{:#x}, {:#x})", stext as usize, etext as usize);
    debug!("[kernel] .rodata [{:#x}, {:#x})", srodata as usize, erodata as usize);
    info!("[kernel] .data [{:#x}, {:#x})", sdata as usize, edata as usize);
    warn!(
        "[kernel] boot_stack top=bottom={:#x}, lower_bound={:#x}",
        boot_stack_top as usize, boot_stack_lower_bound as usize
    );
    error!("[kernel] .bss [{:#x}, {:#x})", sbss as usize, ebss as usize);

    // 5. 通过已提供的 QEMU 退出设备以成功状态结束。
    crate::board::QEMU_EXIT_HANDLE.exit_success();
}
```

求和用固定长度栈数组和 `for` 循环，不引入堆分配。链接符号是地址边界，统一用 `符号 as usize` 取值，不调用、不解引用。

### 4.3 运行与验收

```bash
cd os
make run BASE=2
```

输出（完整见 `reports/ch1-api-run.log`）：

```text
[kernel] Hello, world!
[kernel] sum = 15
```

命令退出码 `0`，QEMU 正常退出，无 panic、无挂起。缺省 `LOG` 下五条布局日志不显示，属正常行为，未修改 Makefile 强制日志级别。

### 4.4 与参考实现的异同

**相同点**

- 初始化顺序一致：都是先 `clear_bss()` 再 `logging::init()`；
- 都打印 `[kernel] Hello, world!`；
- 都以相同的宏、级别、格式与顺序输出五条内存布局日志，地址都取自链接符号；
- 都通过 `QEMU_EXIT_HANDLE.exit_success()` 成功退出，不用 `panic!`/`sbi::shutdown()`；
- 都不使用 `std`、堆分配和用户态系统调用。

**不同点**

- **新增计算任务**：参考实现**没有**局部数组求和，也没有 `[kernel] sum = ...` 这一行输出；这是本实验新增的要求。本实现在打印 `Hello, world!` 之后新增了数组定义、遍历求和与结果输出。
- 实现思路：求和采用显式 `for` 循环配合 `numbers.iter()` 与 `sum += *number`，直观体现"实际遍历元素"；也可以改用迭代器 `sum::<usize>()`，功能等价，这里选择循环是为让计算过程在调试时可见。
- 参考实现把 `println!` 放在日志之前，本实现保持相同顺序，仅在两者之间插入求和步骤。

对比围绕功能、执行顺序与实现思路，而非代码文本差异：两者的运行时骨架完全相同，差别只在参考实现缺少本实验要求的求和输出环节。

## 5. 主要问题与解决思路

### 5.1 NixOS 下没有 `rustup`，`make run` 在 `env` 目标失败

- **现象**：执行官方验收命令 `make run BASE=2` 时，`env` 目标报 `sh: rustup: command not found`，`make` 以 `Error 127` 退出。
- **原因分析**：原 `os/Makefile` 的 `env` 目标用 `rustup target add`、`cargo install cargo-binutils`、`rustup component add` 安装工具链；本机是 NixOS，工具链、target、`rust-src`、`cargo-binutils` 都由 flake 的 dev shell 提供，没有 `rustup`。
- **解决思路**：在用户允许的前提下适配 `Makefile`：当 `command -v rustup` 不存在时跳过 rustup 相关步骤，并在 `rust-objcopy` 已存在时跳过 `cargo install cargo-binutils`；有 rustup 的环境行为不变。
- **处理结果**：`make run BASE=2` 正常工作，输出 `Hello, world!` 与 `sum = 15`，退出码 0。

### 5.2 没有 `riscv64-unknown-elf-gdb`

- **现象**：`Makefile` 的 `debug`/`gdbclient` 目标硬编码 `riscv64-unknown-elf-gdb`，本机未安装该名称的调试器。
- **原因分析**：本机提供的是支持全部目标的系统 `gdb 17.2`（`--enable-targets=all`），可直接以 `set architecture riscv:rv64` 调试 RISC-V。
- **解决思路**：在 `Makefile` 增加 `GDB ?= riscv64-unknown-elf-gdb`，NixOS 上用 `gdb` 覆盖；GDB 会话按分析文档手动连接。
- **处理结果**：GDB 正常连接并完成全部跟踪，日志见 `reports/ch1-gdb.log`。

### 5.3 `LOG=TRACE` 的生效条件

- **现象**：仅运行内核不会显示布局日志。
- **原因分析**：`option_env!("LOG")` 在编译期读取，未设 `LOG` 时日志级别为 `Off`。
- **解决思路**：构建时传入 `LOG=TRACE`。GNU make 会把命令行变量放进 recipe 环境，cargo 也会跟踪 `option_env!` 依赖并在该变量变化时重建。
- **处理结果**：debug 构建中五条日志均可见；验收用默认构建不显示，符合文档说明。

### 5.4 闭包参数 `a` 的 DWARF 取值

- **现象**：在 `clear_bss` 闭包入口，GDB `info args` 显示的 `a=2149679527`（`0x802181a7`），**并非** `sbss`；而分析文档描述首次进入时 `a == sbss`。
- **原因分析**：该闭包被 GDB 记为 `os::clear_bss::{closure#0}(*mut closure_env, usize)`，第一个参数是零大小类型的环境指针，DWARF 把它标注为 `a`。真正的元素地址在第二个参数（`a1`）中。
- **解决思路**：不依赖入口处可疑的 `a`，改以真正的写指令为准：在 `sb a0,0(a1)` 处读取寄存器，看到 `a1=0x80219000=sbss`、`a0=0`。
- **处理结果**：BSS 清零证据成立，已在 `reports/ch1-gdb.log` 第 4 段记录。

### 5.5 函数入口处读 `exit` 的实参不准

- **现象**：在 `exit` 函数第一条指令处 `info args`/`p/x self->addr` 得到 `self=0x80218b18, code=0, self->addr=2`，与预期不符。
- **原因分析**：此时函数序言尚未建立栈帧，DWARF 无法定位已保存的参数副本，GDB 读到了未初始化/旧值。
- **解决思路**：改为在写好设备地址、即将执行 `sw` 的指令处（`0x802026ce`）观察，此时调用帧信息完整。
- **处理结果**：该处得到 `code=0x5555`、`self=0x80206428`、`self->addr=0x100000`，与源码 `EXIT_SUCCESS` 和 `VIRT_TEST=0x100000` 一致。

### 5.6 MMIO 写导致 GDB 断开

- **现象**：单步执行 `sw a0,0(a1)` 后 GDB 报 `Remote connection closed`。
- **原因分析**：该写触发 QEMU 退出，远端 QEMU 进程结束，调试连接随之关闭。
- **解决思路**：把断开视为正常退出的一部分，并另行确认终端退出码为 0、输出含 `Hello, world!` 与 `sum = 15`。
- **处理结果**：成功退出链路完整闭合。

## 6. 结论

参考实现的启动流程可概括为：固件把内核加载到 `0x80200000`，`_start` 建立 64 KiB 启动栈后调用 `rust_main()`；`rust_main()` 先清 BSS 再初始化日志，打印启动信息，按五级日志输出内存布局，最后通过 QEMU 退出设备以 `0x5555` 成功结束。GDB 跟踪在以上每一环节都取得了具体证据（栈上下界、`sb` 写指令、`ecall` 参数、MMIO 写），四条调用链均得到验证。

独立实现在保持相同运行时骨架的前提下，按契约补充了参考实现所没有的"局部数组遍历求和并输出"任务，`make run BASE=2` 输出 `Hello, world!` 与 `sum = 15` 并以退出码 0 结束。实验中遇到的主要障碍集中在 NixOS 环境适配（`rustup`、`gdb`）与 GDB 观察细节（闭包参数、函数入口值）上，均已定位并解决。
