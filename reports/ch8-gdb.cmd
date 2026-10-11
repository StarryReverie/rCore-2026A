# ch8 GDB session scripts (reference kernel, MODE=debug BASE=2)
#
# Build once:  cd os && make build MODE=debug BASE=2
#
# Each scene drives the user shell through qemu stdio and traces one test app.
# The debug kernel is started halted (-s -S); gdb attaches and continues.
# Use a FIFO for stdin so the app name is typed after the shell is up:
#
#   cd os
#   FIFO=/tmp/qemu-in; rm -f $FIFO; mkfifo $FIFO
#   ( sleep 15; printf 'ch8b_test_condvar\n'; sleep 55 ) > $FIFO &
#   qemu-system-riscv64 -m 128M -machine virt -nographic -bios ../bootloader/rustsbi-qemu.bin \
#     -device loader,file=target/riscv64gc-unknown-none-elf/debug/os.bin,addr=0x80200000 \
#     -drive file=../user/target/riscv64gc-unknown-none-elf/release/fs.img,if=none,format=raw,id=x0 \
#     -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -s -S < $FIFO &
#   timeout 80 gdb -nx -batch -q target/riscv64gc-unknown-none-elf/debug/os -x SceneA
#
# Scenes:
#   A  ch8b_test_condvar          -> sys_mutex_lock, MutexBlocking lock/unlock, Condvar wait/signal
#   B  ch8b_sync_sem              -> Semaphore down/up (count 0)
#   C  ch8b_race_adder_mutex_spin -> MutexSpin lock/unlock contention
#   D  ch8b_phil_din_mutex        -> MutexBlocking lock->block and unlock->wakeup_task
#
# Note: Scene C's heavy 16-thread spin app traps (LoadPageFault) in the debug
# kernel while gdb is attached; without gdb it passes. Scene C below therefore
# only records lock entries. NixOS has no riscv64-unknown-elf-gdb; the
# multiarch `gdb` 17.2 is used. Prints use the UPSafeCell/RefCell/UnsafeCell
# layout: <obj>.inner.inner.value.value.<field>.

################ Scene A: ch8b_test_condvar ################
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch8-gdb.log
set logging overwrite on
set logging enabled on
target remote localhost:1234

set $ml = 0
break os::syscall::sync::sys_mutex_lock
commands
  silent
  set $ml = $ml + 1
  if $ml <= 3
    printf "\n===== [A1] sys_mutex_lock #%d mutex_id=%d =====\n", $ml, mutex_id
    bt 4
  end
  continue
end

set $bl = 0
break 'os::sync::mutex::{impl#3}::lock'
commands
  silent
  set $bl = $bl + 1
  if $bl <= 3
    printf "\n===== [A2] MutexBlocking::lock #%d =====\n", $bl
    bt 4
    printf "locked=%d wait_queue_len=%d\n", self->inner.inner.value.value.locked, self->inner.inner.value.value.wait_queue.len
  end
  continue
end

set $cw = 0
break os::sync::condvar::Condvar::wait
commands
  silent
  set $cw = $cw + 1
  if $cw <= 3
    printf "\n===== [A3] Condvar::wait #%d =====\n", $cw
    bt 4
    printf "condvar_wait_queue_len=%d\n", self->inner.inner.value.value.wait_queue.len
  end
  continue
end

set $cs = 0
break os::sync::condvar::Condvar::signal
commands
  silent
  set $cs = $cs + 1
  if $cs <= 3
    printf "\n===== [A4] Condvar::signal #%d =====\n", $cs
    bt 4
    printf "condvar_wait_queue_len=%d\n", self->inner.inner.value.value.wait_queue.len
  end
  continue
end

set $bu = 0
break 'os::sync::mutex::{impl#3}::unlock'
commands
  silent
  set $bu = $bu + 1
  if $bu <= 3
    printf "\n===== [A5] MutexBlocking::unlock #%d =====\n", $bu
    bt 4
    printf "locked=%d wait_queue_len=%d\n", self->inner.inner.value.value.locked, self->inner.inner.value.value.wait_queue.len
  end
  continue
end

continue

################ Scene B: ch8b_sync_sem ################
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch8-gdb.log
set logging overwrite off
set logging enabled on
target remote localhost:1234

set $dn = 0
break src/sync/semaphore.rs:49
commands
  silent
  set $dn = $dn + 1
  if $dn <= 2
    printf "\n===== [B1] Semaphore::down #%d entry =====\n", $dn
    bt 4
    printf "count=%d wait_queue_len=%d\n", self->inner.inner.value.value.count, self->inner.inner.value.value.wait_queue.len
  end
  continue
end

set $bk = 0
break src/sync/semaphore.rs:59
commands
  silent
  set $bk = $bk + 1
  if $bk <= 2
    printf "\n===== [B2] Semaphore::down -> block #%d =====\n", $bk
    bt 3
    printf "count=%d wait_queue_len=%d\n", self->inner.inner.value.value.count, self->inner.inner.value.value.wait_queue.len
  end
  continue
end

set $up = 0
break src/sync/semaphore.rs:35
commands
  silent
  set $up = $up + 1
  if $up <= 2
    printf "\n===== [B3] Semaphore::up #%d entry =====\n", $up
    bt 4
    printf "count=%d wait_queue_len=%d\n", self->inner.inner.value.value.count, self->inner.inner.value.value.wait_queue.len
  end
  continue
end

set $wk = 0
break src/sync/semaphore.rs:43
commands
  silent
  set $wk = $wk + 1
  if $wk <= 2
    printf "\n===== [B4] Semaphore::up -> wakeup_task #%d =====\n", $wk
    bt 3
    printf "count=%d wait_queue_len=%d\n", self->inner.inner.value.value.count, self->inner.inner.value.value.wait_queue.len
  end
  continue
end

continue

################ Scene C: ch8b_race_adder_mutex_spin ################
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch8-gdb.log
set logging overwrite off
set logging enabled on
target remote localhost:1234

printf "\n########## Scene C: ch8b_race_adder_mutex_spin (spin mutex) ##########\n"
printf "NOTE: the debug kernel traps (LoadPageFault) while GDB is attached to this\n"
printf "heavy 16-thread app; the app passes without GDB (see ch8-api-impl-run.log).\n"

set $sl = 0
break src/sync/mutex.rs:41
commands
  silent
  set $sl = $sl + 1
  if $sl <= 3
    printf "\n===== [C1] MutexSpin::lock #%d entry =====\n", $sl
    bt 4
    printf "locked=%d\n", self->locked.inner.value.value
  end
  continue
end

continue

################ Scene D: ch8b_phil_din_mutex ################
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch8-gdb.log
set logging overwrite off
set logging enabled on
target remote localhost:1234

set $pl = 0
break src/sync/mutex.rs:99
commands
  silent
  set $pl = $pl + 1
  if $pl <= 4
    printf "\n===== [D1] MutexBlocking::lock #%d entry =====\n", $pl
    bt 4
    printf "locked=%d wait_queue_len=%d\n", self->inner.inner.value.value.locked, self->inner.inner.value.value.wait_queue.len
  end
  continue
end

set $pb = 0
break src/sync/mutex.rs:106
commands
  silent
  set $pb = $pb + 1
  if $pb <= 3
    printf "\n===== [D2] MutexBlocking::lock -> block #%d =====\n", $pb
    bt 3
    printf "locked=%d wait_queue_len=%d\n", self->inner.inner.value.value.locked, self->inner.inner.value.value.wait_queue.len
  end
  continue
end

set $pu = 0
break src/sync/mutex.rs:114
commands
  silent
  set $pu = $pu + 1
  if $pu <= 4
    printf "\n===== [D3] MutexBlocking::unlock #%d entry =====\n", $pu
    bt 4
    printf "locked=%d wait_queue_len=%d\n", self->inner.inner.value.value.locked, self->inner.inner.value.value.wait_queue.len
  end
  continue
end

set $wk = 0
break src/sync/mutex.rs:121
commands
  silent
  set $wk = $wk + 1
  if $wk <= 3
    printf "\n===== [D4] MutexBlocking::unlock -> wakeup_task #%d =====\n", $wk
    bt 3
    printf "locked=%d wait_queue_len=%d\n", self->inner.inner.value.value.locked, self->inner.inner.value.value.wait_queue.len
  end
  continue
end

continue
