# 第 8 章 线程同步与死锁检测 实验报告

## 0. 实验对象与环境

| 项 | 内容 |
| --- | --- |
| 分析对象 | `ch8` 分支参考实现（`os/src/sync/{mutex,semaphore,condvar,deadlock}.rs`、`os/src/syscall/sync.rs`、`os/src/task`） |
| 独立实现 | `ch8-api` 分支：`mutex.rs` 4 处 + `semaphore.rs` 2 处 + `condvar.rs` 2 处 + `deadlock.rs` 1 处，共 9 处 TODO |
| 构建 | `make build BASE=2`；GDB 跟踪用 `make build MODE=debug BASE=2`（用户程序与 fs 镜像保持 release） |
| 运行/验收 | `make run BASE=2`，在用户 shell 内运行 `ch8_usertest` |
| 调试器 | NixOS 无 `riscv64-unknown-elf-gdb`；用多架构 `gdb` 17.2，QEMU 侧 `-s -S` |
| 日志 | `reports/ch8-gdb.log`（参考实现跟踪）、`reports/ch8-api-impl-run.log`（实现验收）；脚本 `reports/ch8-gdb.cmd` |

GDB 会话固定设置：

```gdb
set pagination off
set confirm off
set architecture riscv:rv64
set language c
target remote localhost:1234
```

`UPSafeCell` 内部是 `RefCell`，`RefCell` 再包 `UnsafeCell`，因此 GDB 打印字段的路径是
`<对象>.inner.inner.value.value.<字段>`（例如 `self->inner.inner.value.value.locked`）。

---

## 1. 静态分析

### 1.1 四层分工

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 系统调用层 | `syscall/sync.rs` | 取对象、做 `request()` 拒绝判定、把布尔/`-0xDEAD` 返回给用户 |
| 同步原语层 | `sync/{mutex,semaphore,condvar}.rs` | `locked` / `count` / `wait_queue` 状态机，等待与交接 |
| 调度层 | `task::{mod,manager,processor}.rs` | `suspend`（Ready 回队）、`block`（Blocked 不入队）、`wakeup_task`（Ready 入队，不立即切换） |
| 检测器层 | `sync/deadlock.rs` | `available/allocation/need` 记账与 `is_safe` 安全判定；每进程互斥锁、信号量各一个检测器 |

互斥锁和信号量的检测器分开，不做跨类型环；创建对象时 `Resource::new` 在对应检测器里登记一列
并初始化 `available`。

### 1.2 任务状态与两种“让出”

- `suspend_current_and_run_next()`：当前线程置 `Ready` 并放回就绪队列，用于自旋锁让权。
- `block_current_and_run_next()`：当前线程置 `Blocked` 且不放回队列，用于阻塞锁、信号量、条件变量。
- `wakeup_task(task)`：把 `task` 置 `Ready` 并加入就绪队列，但不立即切换——交接待被唤醒者下次调度时完成。

`UPSafeCell` 是动态独占借用（`RefCell`），跨任务切换前必须释放，否则另一线程访问同一对象会重入借用。
单核且内核态不抢占，因此“检查—入队—阻塞”之间不会插入其他线程，不存在丢失唤醒窗口。

### 1.3 `Resource` 记账

| 方法 | 行为 |
| --- | --- |
| `wait()` | 取当前线程 `tid`，确保行存在，`need[tid]=Some(resource_id)` |
| `request()` | 先 `wait()`；`enabled && !is_safe()` 时清除 `need[tid]` 返回 `false` |
| `acquire(task)` | `available[id]-=1`、`allocation[tid][id]+=1`、清除该线程 `need` |
| `release()` | 当前线程持有量饱和减一、`available[id]+=1`（不要求当前线程一定持有，以支持信号量 `up` 通知） |

`need` 只保存**一个**当前请求，不是最大需求；`acquire` 一定只在资源确实可用时调用，否则 `usize` 下溢。

### 1.4 互斥锁

**`MutexSpin`**（让权式自旋）：

- `lock()`：`wait()` 登记请求 → 循环取 `locked` 借用；占用则 `drop` 借用 → `suspend` → 重试；空闲则置位并 `acquire`。
- `unlock()`：`release()` 后清 `locked`，无等待队列。

**`MutexBlocking`**（FIFO 阻塞）：

- `lock()`：`wait()` → 空闲则置位并 `acquire`；占用则把当前线程 `push_back` 入队后 `block`。被唤醒后 **不重新判断**，因为 `unlock` 已把所有权直传。
- `unlock()`：`release()` 归还当前线程记账 → 若有等待者，`acquire(队首)` 并把队首 `wakeup_task`，`locked` **保持 `true`**（直传）；仅空队列才清 `locked`。

关键点：`release` 与 `acquire` 成对使 `available` 净不变，锁标志不出现“解锁后又被抢占”的窗口。

### 1.5 信号量

`count` 语义：`>0` 可用许可数；`0` 无许可且无等待；`<0` 的绝对值是等待线程数。负值只存在于 `count`，绝不写入检测器的 `available`。

- `down()`：先 `request()`，被拒直接返回 `false`（不改 `count`/队列）；接受后 `count-=1`；`<0` 入队 `block`（恢复时 `up` 已 `acquire`），`>=0` 立即 `acquire`；返回 `true`。
- `up()`：`release()` 后 `count+=1`；若结果 `<=0`，弹队首 `acquire` 并 `wakeup_task`（不再重复加 `count`）。

`down` 判 `<0`、`up` 判 `<=0` 的不对称来自“先 `-=1` / 先 `+=1`”的顺序，是边界正确性的核心。

### 1.6 条件变量

- `wait(mutex)`：`mutex.unlock()` → 当前线程入 `wait_queue` → `drop` 借用 → `block` → 唤醒后 `mutex.lock()`（可能再次阻塞）。返回时重新持锁，条件需由调用者循环复查。
- `signal()`：FIFO 弹出一个等待者并 `wakeup_task`；空队列是 no-op，不保存通知、不转移锁。

`signal` 只表示“可以再检查条件”，不保证条件成立；与 `MutexBlocking` 配合时，被唤醒者要靠 `wait` 末尾的 `lock()` 重新参与锁竞争。

### 1.7 `DeadlockDetector::is_safe`

对 `available` 做拷贝 `work`，`finish[tid]` 标记可完成线程：

```
work = available.clone()
loop {
  progress = false
  for tid in 0..need.len():
    if !finish[tid] && (need[tid] == None || work[need[tid]] > 0):
      work += allocation[tid]   // 归还已持有
      finish[tid] = true
      progress = true
  if !progress: break
}
return finish.all(true)
```

- 请求的那一单位“获取后最终会释放”，二者相消，所以只加 `allocation`，不要先减再加回（会算错）。
- 空状态安全；只看记账、不修改 `self`、不因 `enabled=false` 跳过计算——是否执行拒绝由 `request()` 决定。
- `sys_enable_deadlock_detect(1)` 时若任一检测器不安全则返回 `-1`，成功才置两个开关。

---

## 2. GDB 动态跟踪

在 `ch8` 参考内核（`MODE=debug BASE=2`）上跟踪，完整日志见 `reports/ch8-gdb.log`，脚本见 `reports/ch8-gdb.cmd`。场景：

- **A** `ch8b_test_condvar`：`sys_mutex_lock`、`MutexBlocking` 加解锁、`Condvar` 等待/通知。
- **B** `ch8b_sync_sem`：`Semaphore::down/up`（初始许可 0）。
- **C** `ch8b_race_adder_mutex_spin`：`MutexSpin` 竞争。
- **D** `ch8b_phil_din_mutex`：`MutexBlocking` 的 `block` 与 `wakeup_task` 交接。

### 2.1 场景 A：条件变量的“解锁—等待—通知—重取锁”

进入 `sys_mutex_lock(mutex_id=0)` 并下探到 `MutexBlocking::lock`：

```
===== [A1] sys_mutex_lock #1 mutex_id=0 =====
#0  os::syscall::sync::sys_mutex_lock (mutex_id=0) at src/syscall/sync.rs:64
#1  os::syscall::syscall (syscall_id=464) at src/syscall/mod.rs:142
#2  os::trap::trap_handler () at src/trap/mod.rs:73

===== [A2] MutexBlocking::lock #1 =====
#0  os::sync::mutex::{impl#3}::lock (self=0x80285990) at src/sync/mutex.rs:100
#1  os::syscall::sync::sys_mutex_lock (mutex_id=0) at src/syscall/sync.rs:83
locked=0 wait_queue_len=0
```

`locked=0`：线程（`second`）拿到空闲锁并 `acquire`。随后另一个线程也来加锁，命中时锁已被占用：

```
===== [A2] MutexBlocking::lock #2 =====
locked=1 wait_queue_len=0
```

`locked=1` 说明进入阻塞分支，将入队并 `block`。`second` 在持有锁时调用条件变量等待：

```
===== [A3] Condvar::wait #1 =====
#0  os::sync::condvar::Condvar::wait (...) at src/sync/condvar.rs:40
#1  os::syscall::sync::sys_condvar_wait (condvar_id=0, mutex_id=0) at src/syscall/sync.rs:253
condvar_wait_queue_len=0            # 入队前

===== [A5] MutexBlocking::unlock #1 =====
#1  os::sync::condvar::Condvar::wait (...) at src/sync/condvar.rs:41   # mutex.unlock()
locked=1 wait_queue_len=1           # 另一线程正等在同一互斥锁上
```

`wait` 的第一件事就是 `mutex.unlock()`（源码第 41 行），此时互斥锁等待队列长度为 1，证明它把锁让给了等待者。`first` 拿到锁后发通知：

```
===== [A4] Condvar::signal #1 =====
#1  os::syscall::sync::sys_condvar_signal (condvar_id=0) at src/syscall/sync.rs:232
condvar_wait_queue_len=1            # 弹出前

===== [A5] MutexBlocking::unlock #2 =====
#1  os::syscall::sync::sys_mutex_unlock (mutex_id=0) at src/syscall/sync.rs:104
locked=1 wait_queue_len=0           # 已直传给被唤醒者
```

通知时队列长度为 1，说明 `second` 确实在条件变量上等待。`first` 解锁时互斥锁队列已空（`wakeup_task` 把 `second` 取出并交接）。`second` 被唤醒后在 `wait` 末尾重新取锁：

```
===== [A2] MutexBlocking::lock #3 =====
#1  os::sync::condvar::Condvar::wait (...) at src/sync/condvar.rs:46   # mutex.lock()
locked=0 wait_queue_len=0
===== [A5] MutexBlocking::unlock #3 =====
#1  os::syscall::sync::sys_mutex_unlock (mutex_id=0) at src/syscall/sync.rs:104
```

**结论**：观察到的调用顺序是 `unlock(41) → wait 入队 → block → signal 弹队 → lock(46)`，与源码
`condvar.rs:41/45/46` 和 `signal` 的“不转移锁”语义一致；`wait` 返回后锁已重新持有。

### 2.2 场景 B：信号量的计数与交接

许可初始为 0。`second` 先 `down`，`first` 睡醒后 `up`：

```
===== [B1] Semaphore::down #1 entry =====
#1  os::syscall::sync::sys_semaphore_down (sem_id=0) at src/syscall/sync.rs:177
count=0 wait_queue_len=0            # 无许可

===== [B2] Semaphore::down -> block #1 =====
#0  os::sync::semaphore::Semaphore::down (...) at src/sync/semaphore.rs:59
count=-1 wait_queue_len=1           # 计数变负，线程入队阻塞

===== [B3] Semaphore::up #1 entry =====
#1  os::syscall::sync::sys_semaphore_up (sem_id=0) at src/syscall/sync.rs:157
count=-1 wait_queue_len=1           # 归还前

===== [B4] Semaphore::up -> wakeup_task #1 =====
#0  os::sync::semaphore::Semaphore::up (...) at src/sync/semaphore.rs:43
count=0 wait_queue_len=0            # 交接完成
```

**结论**：`count` 从 `0 → -1 → 0`，队列长度 `0 → 1 → 0`，正好对应“许可不足时等待、`up` 后交接”；
负数确实表示等待线程数，交接发生在 `up` 内（`acquire(队首) + wakeup_task`），不重复加计数。

### 2.3 场景 C：自旋锁竞争（部分观察）

`ch8b_race_adder_mutex_spin` 让 16 个线程反复竞争同一把 `MutexSpin`：

```
===== [C1] MutexSpin::lock #1 entry =====
#1  os::syscall::sync::sys_mutex_lock (mutex_id=0) at src/syscall/sync.rs:83
locked=0
===== [C1] MutexSpin::lock #2 entry =====
locked=0
===== [C1] MutexSpin::lock #3 entry =====
locked=0
```

锁入口连续命中，说明 `mutex_id=0` 被多线程反复获取。`locked=1` 时源码会走
`mutex.rs:48` 的 `suspend_current_and_run_next()`（置 `Ready` 后重试）。

**限制**：该应用 16 线程高强度让权，debug 内核在 GDB 附加时触发内核陷阱
（`trap_from_kernel`：`LoadPageFault`/`Exception(Unknown)`），未能稳定观察到 `suspend` 处；
而**不附加 GDB** 时同一 debug 内核运行通过（`race adder using spin mutex test passed!`，
见 `ch8-api-impl-run.log`）。因此“`MutexSpin::lock → suspend`”这一环以源码为准，属源码推导。

### 2.4 场景 D：阻塞锁的 `block` 与 `unlock → wakeup_task`

5 个哲学家共享 5 把阻塞锁。加锁入口与前两次进入阻塞分支：

```
===== [D1] MutexBlocking::lock #3 entry =====
#1  os::syscall::sync::sys_mutex_lock (mutex_id=2) at src/syscall/sync.rs:83
locked=1 wait_queue_len=0           # 锁被占用
===== [D2] MutexBlocking::lock -> block #1 =====
#0  os::sync::mutex::{impl#3}::lock (...) at src/sync/mutex.rs:106
locked=1 wait_queue_len=1           # 入队后阻塞
```

`mutex_id=2` 的队列长度从 0 增到 1，随后线程在 `mutex.rs:106` 处 `block`。解锁与交接：

```
===== [D3] MutexBlocking::unlock #1 entry =====
#1  os::syscall::sync::sys_mutex_unlock (mutex_id=2) at src/syscall/sync.rs:104
locked=1 wait_queue_len=1           # 有等待者
===== [D4] MutexBlocking::unlock -> wakeup_task #1 =====
#0  os::sync::mutex::{impl#3}::unlock (...) at src/sync/mutex.rs:121
locked=1 wait_queue_len=0           # 直传给队首，锁仍占用
```

同一模式在 `mutex_id=1`、`mutex_id=3` 上重复出现（见日志 [D3]#2/[D4]#2、[D3]#4/[D4]#3）。
**结论**：`unlock` 在有等待者时先 `release` 再把资源 `acquire` 给队首并 `wakeup_task`，`locked`
始终保持 `true`，只有空队列的 `unlock`（日志 [D3]#3，`mutex_id=4`）才清位。

### 2.5 跟踪结论

- **阻塞与让权**：`MutexSpin` 用 `suspend`（Ready 重试），`MutexBlocking`/`Semaphore`/`Condvar` 用 `block`（Blocked 等待唤醒）。
- **FIFO 与交接**：`MutexBlocking::unlock`、`Semaphore::up` 都“先归还、再给队首”，唤醒一个；锁/许可直接交接。
- **条件变量**：`wait` 严格按 `unlock → 入队 → block → lock` 执行，`signal` 只弹队首、不转锁。
- **记账一致**：观察到的 `locked`、`count`、队列长度变化都能与 `Resource` 的 `wait/request/acquire/release` 对上。

---

## 3. 独立实现对比（`ch8-api`）

`ch8-api` 在相同结构体与辅助函数上完成 9 处接口，算法与 `ch8` 参考实现一致，差异集中在写法与借用顺序。

### 3.1 相同点

- `MutexSpin::lock` 都先 `Resource::wait()` 登记，再循环 `suspend` 重试；`unlock` 都 `release` 后清位。
- `MutexBlocking::lock` 都在占用时 FIFO 入队后 `block`，恢复后不重复 `acquire`；`unlock` 都把所有权直传队首并保持 `locked=true`。
- `Semaphore` 都用带符号 `count`，`down` 先 `request` 后 `count-=1`，`up` 先 `release` 后 `count+=1`，交接不再重复计数。
- `Condvar::wait` 都 `unlock → 入队 → block → lock`；`signal` 都 FIFO 弹一个。
- `is_safe` 都用 `available` 拷贝 + `finish` 标记的银行家模拟，只加 `allocation`、不永久扣减请求。

### 3.2 不同点

| 方面 | 参考 `ch8` | `ch8-api` |
| --- | --- | --- |
| 获取资源时的借用 | 在持有 `locked`/`mutex_inner` 借用时直接调用 `resource.acquire`（`acquire` 不触碰该对象，安全） | 先 `drop(mutex_inner)` 再 `acquire`，借用窗口更保守 |
| `MutexSpin::lock` 重试 | `loop` + `continue`，占用时 `drop` 后 `suspend` | 同样 `loop` + `continue`，先置位并 `drop` 再 `acquire` |
| `is_safe` 完成条件 | `need[tid].map_or(true, \|id\| work[id] > 0)` | 同一表达式的等价写法 |
| 注释/断言 | `assert!(locked)` 保留 | 同样保留 `assert!(locked)` |

这些差异不改变调度语义、FIFO 顺序、锁/许可交接或安全判定结果。

### 3.3 借用与调度

在 `ch8-api` 中，所有 `suspend`/`block` 之前都显式结束对 `self.inner`/`self.locked` 的借用，恢复后再重新取用。
例如 `Semaphore::down` 在入队后立刻 `drop(inner)` 再 `block`；`Condvar::wait` 在 `push_back` 后 `drop(inner)` 再 `block`。
这与场景 A/B/D 中观察到的“让出发生在借用释放之后”一致。

### 3.4 验收

`reports/ch8-api-impl-run.log`（`make run BASE=2`，输入 `ch8_usertest`），全部子测例通过：

- 死锁检测：`deadlock test mutex 1 OK!`、`deadlock test semaphore 1 OK!`、`deadlock test semaphore 2 OK!`
- 同步原语：`test_condvar passed!`、`philosopher dining problem with mutex test passed!`、
  `race adder using spin mutex test passed!`、`sync_sem`/`mpsc_sem` 通过
- 线程：`threads test passed!`、`threads with arg test passed!`
- 末行：`ch8 Usertests passed!`（23 个顶层子测例）

`make build MODE=debug BASE=2` 亦通过，可用于 GDB 跟踪。

---

## 4. 主要问题与解决思路

1. **`todo!` 直接 panic**：未实现时首次 `mutex_create`/`semaphore_create` 就会在同步接口处 panic。
   解决：按“互斥锁 → 信号量 → 条件变量 → 检测器”自底向上实现，先 `make build` 保证类型正确，再运行测例。
2. **借用跨 `block`/`suspend` 重入**：`UPSafeCell` 不可重入，持有 `self.inner` 借用再切换，其他线程访问同一锁会 panic。
   解决：每处切换前 `drop` 借用；GDB 场景 A/B/D 显示让出发生在借用释放之后。
3. **阻塞锁恢复后重复记账**：`MutexBlocking::lock` 被唤醒时若再判断/再 `acquire`，会与 `unlock` 的交接重复。
   解决：恢复后直接返回，所有权由 `unlock` 的 `acquire(队首)` 一次性完成（日志 [D4]）。
4. **解锁把锁错误地清空**：`unlock` 有等待者时若先清 `locked` 再唤醒，会出现窗口。
   解决：有等待者时保持 `locked=true`，仅空队列清位（日志 [D3]/[D4] 中 `locked=1 wait_queue_len 1→0`）。
5. **信号量计数边界错位**：`down`/`up` 的判据写成对称会差一位。
   解决：`down` 先 `count-=1` 再判 `<0`；`up` 先 `count+=1` 再判 `<=0`（日志 [B1]–[B4]）。
6. **条件变量顺序与丢失唤醒**：不先 `unlock` 就入队会让通知者拿不到锁；唤醒后不 `lock` 就返回违反契约。
   解决：严格 `unlock → 入队 → block → lock`（日志 [A3]/[A5]/[A4]/[A2]#3）。
7. **`is_safe` 永久扣减请求**：把请求单位从 `work` 扣掉却不归还，会把安全状态误判为不安全。
   解决：利用“获取与释放相消”，完成时只加 `allocation`，并在拷贝上模拟、不改原记账。
8. **GDB 附加下 debug 内核的内核陷阱**：`ch8b_race_adder_mutex_spin` 在 debug 内核 + GDB 下触发
   `trap_from_kernel`（`LoadPageFault`/`Exception(Unknown)`），不附加 GDB 时通过。
   处理：如实记录该限制，场景 C 只保留锁入口观察，`MutexSpin → suspend` 以源码为准；其余场景在
   debug 内核下均稳定复现。

---

## 5. 附录

- GDB 日志：`reports/ch8-gdb.log`；可复现脚本：`reports/ch8-gdb.cmd`
- 验收运行日志：`reports/ch8-api-impl-run.log`
- 分析参考：`rcore-ch8-analyze.md`；接口契约：`rcore-ch8-api.md`
- 独立实现分支：`ch8-api`，提交 `feat(ch8-api): implement mutex, semaphore and condvar synchronization`
- 线程创建/回收、进程管理、FIFO 调度、定时器和系统调用入口均由课题提供，未计入本次 9 处 TODO。
