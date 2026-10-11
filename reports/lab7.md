# 第 7 章 管道通信与文件描述符共享 实验报告

## 0. 实验对象与环境

| 项 | 内容 |
| --- | --- |
| 分析对象 | `ch7` 分支参考实现（`os/src/fs/pipe.rs`、`os/src/syscall/fs.rs`、`os/src/task`） |
| 独立实现 | `ch7-api` 分支：`os/src/fs/pipe.rs` 7 处 + `os/src/syscall/fs.rs` 2 处 TODO |
| 构建 | `make build BASE=2`；GDB 跟踪用 `make build MODE=debug BASE=2` |
| 运行/验收 | `make run BASE=2`，在用户 shell 内运行 `ch7b_usertest` |
| 调试器 | NixOS 无 `riscv64-unknown-elf-gdb`；用多架构 `gdb` 17.2，QEMU 侧 `-s -S` |
| 日志 | `reports/ch7-gdb.log`、`reports/ch7-api-impl-run.log`；脚本 `reports/ch7-gdb.cmd` |

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

### 1.1 三层对象与分工

| 层 | 位置 | 保存的状态 | 生命周期 |
| --- | --- | --- | --- |
| 环形缓冲区 `PipeRingBuffer` | 内核堆（`Arc<UPSafeCell<..>>`） | `arr[32]`、`head`、`tail`、`status`、`write_end: Option<Weak<Pipe>>` | 被两个端点共享，最后一个端点释放时销毁 |
| 管道端点 `Pipe` | 内核堆 | `readable`、`writable`、`Arc<UPSafeCell<PipeRingBuffer>>` | 每次 `make_pipe` 产生两个；被 fd 表中的 `Arc` 持有 |
| 文件描述符表 | `TaskControlBlockInner` | `Vec<Option<Arc<dyn File + Send + Sync>>>` | 随进程；索引即 fd，`None` 为空闲 |

关键点：**数据在共享缓冲区，权限在端点，引用在 fd 表**。读端 `readable=true/writable=false`，写端相反；两个端点必须持有同一个 `Arc`，不同 `make_pipe` 之间不共享内容。

### 1.2 环形缓冲区与空满区分

- `head` 是下一次读的位置，`tail` 是下一次写的位置，推进后继都 `% RING_BUFFER_SIZE`（32）。
- 空和满都满足 `head == tail`，必须用 `status`（`Empty/Full/Normal`）区分，不能浪费一个槽位：
  - `Empty`：`available_read = 0`、`available_write = 32`；
  - `Full`：`available_read = 32`、`available_write = 0`；
  - `Normal`：可读为 `(tail + 32 - head) % 32`。
- 任意状态恒有 `available_read() + available_write() == 32`，这是本实验的硬性不变式。
- `write_byte`/`read_byte` 只移动一端的位置，保持 FIFO 顺序；每次写完/读完后根据 `tail == head` 或 `head == tail` 维护 `Full`/`Empty`。

### 1.3 端点创建与弱引用（EOF 的根据）

`make_pipe`（参考 `pipe.rs:103-109`）：

1. `Arc::new(UPSafeCell::new(PipeRingBuffer::new()))`，两个端点都 `clone` 这个 `Arc`，保证共享同一缓冲区；
2. 用已提供的 `read_end_with_buffer`/`write_end_with_buffer` 构造权限；
3. 返回前 `buffer.exclusive_access().set_write_end(&write_end)`，缓冲区保存的是写端的 `Weak`，不是强引用。

因此：

- 缓冲区**不会**持有写端强引用，不会形成 `Pipe -> buffer -> Pipe` 的引用环，写端能被正常释放；
- `all_write_ends_closed()` 就是 `write_end.upgrade().is_none()`：所有指向写端的强引用（fd 表项、`fork` 克隆、`dup` 克隆、syscall 临时克隆）都释放后，`Weak` 才失效；
- EOF 判定发生在**读路径**：读端先读空缓冲区，再询问写端是否全部关闭，而不是 `sys_close` 直接触发关闭查询。

### 1.4 阻塞读写

参考 `Pipe::read`（`pipe.rs:118-148`）与 `Pipe::write`（`pipe.rs:149-171`）结构对称：

- `read`：`want = buf.len()`；循环取缓冲区独占借用，`available_read()` 为一轮上限；读空时，若 `all_write_ends_closed()` 成立则返回已读长度（EOF，可为 0 或短读），否则 `drop(borrow)` → `suspend_current_and_run_next()` → 恢复后重查。只有 EOF 才允许短读。
- `write`：循环取借用，`available_write()` 为一轮上限，写满时 `drop(borrow)` → `suspend` → 重查；非空请求必须写完，不因暂时无空间返回短写，也不因容量 32 而把 32 当作返回上限（请求可 >32，需分批）。
- **借用必须在让出 CPU 前释放**：`UPSafeCell` 是动态独占借用，若持有它 `suspend`，另一个进程/中断访问同一管道就会重入借用。
- 参考实现的边界行为：零长度 `read` 若缓冲区为空且写端存活，会进入等待分支而阻塞；零长度 `write` 若缓冲区满，也会阻塞。API 契约要求零长度立即返回 `0`，这是本实验需要明确区分的地方（见 3.3）。

### 1.5 描述符分配与共享

- `alloc_fd()` 返回当前最小的 `None` 索引；没有空槽时在表尾追加 `None` 并返回该索引。它只找位置，调用者随后必须写入对象。
- `sys_pipe`（参考 `syscall/fs.rs:84-97`）：先 `alloc_fd()` 得到 `read_fd` 并**立即占用**写读端，再 `alloc_fd()` 得到 `write_fd`，保证两者不同；结果通过 `translated_refmut` 写回用户地址，不能直接解引用用户虚址。标准输入输出占用 0/1/2 时通常得到 3、4，但不能硬编码。
- `sys_dup`（`syscall/fs.rs:99-113`）：越界或 `None` 返回 `-1`；否则克隆原表项的 `Arc` 存入新槽。`dup` 不重开文件、不复制偏移或缓冲区，普通文件共享同一偏移，管道共享同一端点，从而延长写端强引用寿命、推迟 EOF。
- `fork`（`task.rs:268-273`）逐项 `clone` fd 表里的 `Arc`，父子共享文件对象；`exec` 保留 fd 表，可把管道传给新程序；`close` 只 `take` 一项；`exit` 清空整表（`task/mod.rs:99`）。因此 EOF 取决于所有写端强引用是否释放，而非某个进程是否调用过 `close`。

---

## 2. GDB 动态跟踪

在 `ch7` 参考内核（`MODE=debug BASE=2`）上跟踪，完整日志见 `reports/ch7-gdb.log`，脚本见 `reports/ch7-gdb.cmd`。两个场景：

- **场景 A**：`ch7b_pipetest`（父子 13 字节传输、关闭写端、EOF 短读）＋ shell 重定向验证 `sys_dup`。
- **场景 B**：`ch7b_pipe_large_test`（3000 字节跨容量传输，观察分批与回绕）。

### 2.1 管道创建：`sys_pipe → make_pipe → set_write_end`

`ch7b_pipetest` 进入 `sys_pipe`（syscall 59），`pipe` 是用户虚址：

```
===== [A1] sys_pipe entry (ch7b_pipetest) =====
#0  os::syscall::fs::sys_pipe (pipe=0xded8) at src/syscall/fs.rs:85
#1  os::syscall::syscall (syscall_id=59, args=...) at src/syscall/mod.rs:78
#2  os::trap::trap_handler () at src/trap/mod.rs:73
```

紧接着命中 `make_pipe` 与 `set_write_end`：

```
Temporary breakpoint 2, os::fs::pipe::make_pipe () at src/fs/pipe.rs:104
#1  os::syscall::fs::sys_pipe (pipe=0xded8) at src/syscall/fs.rs:89

Temporary breakpoint 3, os::fs::pipe::PipeRingBuffer::set_write_end (self=0x82273b98, write_end=...) at src/fs/pipe.rs:62
#1  os::fs::pipe::make_pipe () at src/fs/pipe.rs:107
```

调用链与源码一致：`sys_pipe` 先产出两个端点（共享同一缓冲区），`make_pipe` 返回前把写端以 `Weak` 注册进缓冲区（`set_write_end` 对应 `pipe.rs:107` 的注册，`pipe.rs:62` 的 `Arc::downgrade`）。`ch7b_pipetest` 随后断言 `pipe_fd == [3, 4]`，说明两个描述符不同。

### 2.2 等待：写端存活时读端让出 CPU

在父进程写入之前，子进程先 `read`。`read` 发现缓冲区为空，于是查询写端状态：

```
===== [A6] all_write_ends_closed #1 =====
#0  os::fs::pipe::PipeRingBuffer::all_write_ends_closed (self=0x82273b98) at src/fs/pipe.rs:98
#1  os::fs::pipe::{impl#2}::read (self=0x80276510, buf=...) at src/fs/pipe.rs:127
#2  os::syscall::fs::sys_read (fd=3, buf=0xdee8, len=32) at src/syscall/fs.rs:49
```

`all_write_ends_closed` 连续命中 #1–#3，说明阻塞发生在读端：此时父进程仍持有写端，`upgrade()` 成功，读端 `drop` 借用并 `suspend`，反复重查直到有数据或写端关闭。这正对应 `pipe.rs:126-132`。

### 2.3 写入：FIFO 与状态维护

父进程 `sys_write(fd=4, len=13)` 进入 `Pipe::write`，逐字节写入：

```
===== [A4] write_byte #1 byte=72 =====
#0  os::fs::pipe::PipeRingBuffer::write_byte (self=0x82273b98, byte=72 'H') at src/fs/pipe.rs:65
#1  os::fs::pipe::{impl#2}::write (self=0x80276530, buf=...) at src/fs/pipe.rs:165
#2  os::syscall::fs::sys_write (fd=4, buf=0x4320, len=13) at src/syscall/fs.rs:27
head=0 tail=0 status=1

===== [A4] write_byte #2 byte=101 =====
head=0 tail=1 status=2
... （#13 byte=33 '!'）head=0 tail=12 status=2
```

`write_byte #1` 打印的是执行前状态 `status=1`（`Empty`）；写入 1 字节后 `tail` 前进、状态转为 `2`（`Normal`）。`'H'`=72、`'e'`=101、`'!'`=33 与 `"Hello, world!"` 的顺序一致，验证 FIFO。

### 2.4 读取与 EOF

子进程被唤醒后逐字节消费：

```
===== [A5] read_byte #1 =====
#1  os::fs::pipe::{impl#2}::read (self=0x80276510, buf=...) at src/fs/pipe.rs:137
head=0 tail=13 status=2
... （#13）head=12 tail=13 status=2
```

读到第 13 字节后，`ch7b_pipetest` 的孩子关闭自己的写端副本、父进程也关闭写端，读端再次进入 `all_write_ends_closed`：

```
===== [A6] all_write_ends_closed #4/#5 =====
#1  os::fs::pipe::{impl#2}::read ... at src/fs/pipe.rs:127
```

这次 `upgrade()` 失败返回真，`read` 在把残留数据读完后以 EOF 结束（返回已读长度）。可见**关闭端点（引用计数下降）与查询 EOF 是先后事件**：`sys_close` 只移除 fd 表项、释放强引用，真正的关闭判定发生在之后的 `read → all_write_ends_closed`。

### 2.5 3000 字节的分批与回绕

`ch7b_pipe_large_test` 中孩子一次 `read(down_pipe_fd[0], 3000)`，父亲一次 `write(down_pipe_fd[1], 3000)`：

```
===== [B2] Pipe::read entry =====
#0  os::fs::pipe::{impl#2}::read ... at src/fs/pipe.rs:119
#1  os::syscall::fs::sys_read (fd=3, buf=0xc300, len=3000) at src/syscall/fs.rs:49

===== [B1] Pipe::write entry =====
#0  os::fs::pipe::{impl#2}::write ... at src/fs/pipe.rs:150
#1  os::syscall::fs::sys_write (fd=4, buf=0xc300, len=3000) at src/syscall/fs.rs:27
```

`write` 以 `available_write()` 为每轮上限反复推进，`write_byte` 计数显示每 32 字节回绕一次：

```
===== [B3] write_byte #1  =====  head=0 tail=0 status=1   （空 -> 写第 1 批）
===== [B3] write_byte #33 =====  head=0 tail=0 status=1   （上一批 32 已被读空，tail 回绕到 0）
===== [B3] write_byte #65 =====  head=0 tail=0 status=1   （第三批开始）
```

`tail` 在 0→32（即 0）间回绕、每批开始时缓冲区已被读端排空，说明内核把 3000 字节拆成多批写入，而不是覆盖或一次性写完；`read` 侧则累计到 3000 才返回（测例断言 `read == LENGTH`）。场景 B 中读端较快，未观察到“满”等待；而场景 A 的 #1–#3 已展示“暂时无数据”时的等待。

### 2.6 `sys_dup`

shell 执行 `ch2b_hello_world > audit7` 时把标准输出重定向到文件，命中：

```
===== [A7] sys_dup (shell redirection) =====
#0  os::syscall::fs::sys_dup (fd=3) at src/syscall/fs.rs:100
#1  os::syscall::syscall (syscall_id=24, args=...) at src/syscall/mod.rs:75
$2 = 3
```

`sys_dup(fd=3)` 克隆已打开文件的 `Arc`，新描述符随后被装到 fd 1（shell 关闭了原 fd 1 再复制），共享同一打开偏移，因此后续 `ch7b_cat audit7` 能读到内容（运行日志见 3.5）。

### 2.7 跟踪结论

- **共享与权限**：`make_pipe` 让两个端点共享唯一缓冲区，缓冲区只存写端 `Weak`。
- **阻塞与调度**：`read`/`write` 在条件不满足时先释放独占借用再 `suspend`，恢复后重查；短读只发生在 EOF。
- **EOF 生命周期**：`all_write_ends_closed` 由读路径调用，取决于写端强引用是否全部释放，与 `close` 的调用时刻解耦。
- **分批**：3000 字节按 32 字节容量循环回绕，读写各自累计到请求长度。

---

## 3. 独立实现对比（`ch7-api`）

`ch7-api` 在同一组结构体与辅助函数之上完成 9 处接口，总体算法与 `ch7` 参考实现一致。

### 3.1 相同点

- **环形状态**：都用 `head/tail` + `Full/Empty/Normal`，写入/读取后按位置相等更新状态，不牺牲槽位，`available_read + available_write == 32`。
- **弱引用与 EOF**：`make_pipe` 都只用 `Arc::downgrade` 注册写端；`all_write_ends_closed` 都用 `upgrade().is_none()` 判定。
- **阻塞循环**：都在借用到 `available_*` 为零时先 `drop` 借用再 `suspend_current_and_run_next()`，恢复后重查；都在 `read` 侧用 `all_write_ends_closed` 判 EOF。
- **分批与短读**：都以缓冲区容量为单轮上限、按请求总长度累计返回，`read` 只在 EOF 短读。
- **描述符**：`sys_pipe` 先占读端槽位再分配写端；`sys_dup` 克隆 `Arc`，不做重开/复制。

### 3.2 不同点

| 方面 | 参考 `ch7` | `ch7-api` |
| --- | --- | --- |
| 零长度读写 | 无特判：`read` 空且写端存活会阻塞，`write` 满时会阻塞；`read` 有数据时甚至会消费字节 | 进入循环前 `if want == 0 { return 0 }`，立即返回，不等待也不消费 |
| `available_read` | `status==Empty` 特判，其余用 `tail > head` 分支，否则 `tail+32-head` | `match status`：`Empty=>0`、`Full=>32`、`Normal=>(tail+32-head)%32`，直接覆盖 `head==tail` 的满/空两种情况 |
| `available_write` | `Full?0:32-available_read()` | `32 - available_read()`，由不变式自然覆盖 `Full` |
| 可读性断言 | `assert!(self.readable())`/`writable()` | 保留相同断言 |
| `sys_pipe` 结果写回 | `*translated_refmut(token, unsafe { pipe.add(1) })` | `(pipe as usize + size_of::<usize>()) as *mut usize` 后 `translated_refmut` |
| `sys_dup` 克隆 | 直接在赋值语句内 `Arc::clone(inner.fd_table[fd]...)` | 先 `let file = ...clone();` 再写入新槽，借用更直观 |

差异集中在边界处理与写法，不改变环形数据结构、描述符语义、FIFO 顺序或 EOF 时机。

### 3.3 零长度契约

API 文档明确要求：零长度 `read`/`write` 立即返回 `0`。参考实现没有该特判，其 `read` 在缓冲区为空且写端存活时会进入等待分支，违背“零长度不阻塞”的契约；`ch7-api` 在函数开头处理：

```rust
let want_to_read = buf.len();
if want_to_read == 0 {
    return 0;
}
```

`write` 同理。由于 `UserBuffer::len()` 在零长度时没有切片，参考实现还可能因迭代器与状态组合产生未定义行为；显式早返回避免了这一问题。

### 3.4 借用释放与重入

`UPSafeCell::exclusive_access()` 返回动态独占借用。实现中所有 `suspend_current_and_run_next()` 之前都先 `drop(ring_buffer)`：

```rust
if loop_read == 0 {
    if ring_buffer.all_write_ends_closed() {
        return already_read;
    }
    drop(ring_buffer);
    suspend_current_and_run_next();
    continue;
}
```

`suspend` 会切换到其他进程，而其他进程可能持有同一管道的端点；若借用未释放，恢复借用就会重入同一 `UPSafeCell`。这一点在 GDB 场景 A 的 #1–#3 中得到印证：阻塞发生在释放借用之后。

### 3.5 验收

`reports/ch7-api-impl-run.log`（`make run BASE=2`，输入 `ch7b_usertest`）：

- `ch7b_pipetest`：`Read OK, child process exited!`、`pipetest passed!`，退出码 `0`。
- `ch7b_pipe_large_test`：`pipe_large_test passed!`、`sum = 54000(parent)`/`sum = 54000(child)`，退出码 `0`。
- `ch4b_sbrk`：退出码 `-11`（第 7 章按 `SIGSEGV` 处理，符合预期）；其余顶层子测例退出码 `0`。
- 末行 `Basic usertests passed!`。

补充：在 shell 中执行 `ch2b_hello_world > audit7` 后 `ch7b_cat audit7`，正确输出 `Hello, world from user mode program!`，验证 `sys_dup` 与重定向路径。

`make build MODE=debug BASE=2` 亦通过，可用于 GDB 跟踪。

---

## 4. 主要问题与解决思路

1. **`todo!` 直接 panic**：未实现时首次创建管道会在 `sys_pipe` 处 panic，后续依赖环形与描述符接口。
   解决：按 9 处接口自底向上实现——先缓冲区原语，再 `make_pipe`，再阻塞读写，最后两个 syscall；先 `make build` 保证类型正确，再运行测例。
2. **借用跨 `suspend` 重入**：`UPSafeCell` 不可重入，持有借用让出 CPU 后再访问同一缓冲区会 panic。
   解决：在每处 `suspend_current_and_run_next()` 前 `drop(ring_buffer)`，恢复后 `continue` 重新取借用（`pipe.rs` 读/写两处）。
3. **`head == tail` 的空满区分**：若只比较位置会把满当成空，或浪费一个容量。
   解决：用 `status` 明确 `Empty/Full/Normal`；`available_read` 用 `match status` 计算，`available_write = 32 - available_read`，保证每状态和恒为 32。
4. **描述符顺序与共享**：`alloc_fd` 只找空位，连续两次不占用会得到同一 fd。
   解决：`sys_pipe` 取得 `read_fd` 后**先写入**读端再 `alloc_fd` 取 `write_fd`；`sys_dup` 先克隆 `Arc` 再写入新槽，不改原表项。
5. **3000 字节被当成短读**：把 `available_read()` 当作单次返回长度，或暂时读空就返回，会让 `ch7b_pipe_large_test` 断言失败。
   解决：缓冲区容量只作每轮上限，`want = buf.len()` 决定返回；写端存活且暂时读空时 `suspend` 后继续累计，直到填满或 EOF（GDB 场景 B 显示 32 字节/批的循环回绕）。
6. **EOF 无法到达或提前到达**：缓冲区若保存写端强引用，`Weak` 永不失效；若用某个进程是否 `close` 判断 EOF，则 `fork`/`dup` 的副本会导致误判。
   解决：只存 `Weak`，由读路径 `all_write_ends_closed()` 判定；GDB 中 `close` 与 `all_write_ends_closed` 是先后事件，多次命中后才返回真。
7. **零长度阻塞**：参考实现的零长度读写会进入空/满等待分支。
   解决：在 `read`/`write` 开头对 `want == 0` 立即返回 `0`，满足 API 契约（见 3.3）。

---

## 5. 附录

- GDB 日志：`reports/ch7-gdb.log`；脚本：`reports/ch7-gdb.cmd`
- 验收运行日志：`reports/ch7-api-impl-run.log`
- 分析参考：`rcore-ch7-analyze.md`；接口契约：`rcore-ch7-api.md`
- 独立实现分支：`ch7-api`，提交 `feat(ch7-api): implement pipe communication and descriptor duplication`
- 配套的进程创建、地址转换、普通文件、标准输入输出、信号处理均由课题提供，未计入本次 9 处 TODO。
