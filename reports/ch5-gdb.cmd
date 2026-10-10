# ch5 GDB session scripts (reference kernel, MODE=debug BASE=2)
# Run: make gdbserver MODE=debug BASE=2   # in terminal 1 (NixOS: add GDB=gdb)
#      gdb -nx -batch -q target/riscv64gc-unknown-none-elf/debug/os -x reports/ch5-gdb.cmd

################ Scene 1: process copy (sys_fork) ################
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch5-gdb.log
set logging overwrite on
set logging enabled on
target remote localhost:1234
break os::syscall::process::sys_fork
continue
printf "\n===== [1] breakpoint hit: os::syscall::process::sys_fork =====\n"
bt 8
info registers pc sp
break src/syscall/process.rs:54
continue
printf "\n===== [2] back in sys_fork after TaskControlBlock::fork(), before child a0 reset =====\n"
bt 4
info locals
set language rust
printf "\n-- parent pid = "
print (*current_task.ptr.pointer).data.pid.0
printf "-- child  pid = "
print (*new_task.ptr.pointer).data.pid.0
printf "-- child kernel_stack id = "
print (*new_task.ptr.pointer).data.kernel_stack.0
printf "-- child task_cx (goto trap_return) = "
print (*new_task.ptr.pointer).data.inner.inner.value.value.task_cx
set language c
printf "\n-- child TrapContext copied from parent --\n"
print *trap_cx
printf "-- child trap_cx kernel_sp = %p\n", trap_cx->kernel_sp
printf "-- child trap_cx x[10] BEFORE reset = %ld\n", trap_cx->x[10]
next
printf "-- child trap_cx x[10] AFTER reset  = %ld  (fork returns 0 in child)\n", trap_cx->x[10]
kill
quit

################ Scene 2: stride scheduling (TaskManager::fetch) ################
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch5-gdb.log
set logging overwrite off
set logging enabled on
set $n = 0
target remote localhost:1234
break src/task/manager.rs:35
commands
  silent
  set $n = $n + 1
  printf "\n===== TaskManager::fetch selection #%d =====\n", $n
  if $n == 1
    printf "-- call stack: idle loop -> fetch_task -> TaskManager::fetch --\n"
    bt 5
  end
  set language rust
  print index
  print (*task.ptr.pointer).data.pid.0
  print (*task.ptr.pointer).data.inner.inner.value.value.stride
  print (*task.ptr.pointer).data.inner.inner.value.value.prio
  set language c
  if $n < 8
    continue
  end
end
continue
printf "\n(stop after 8 selections; each later selected process is charged BIG_STRIDE/prio)\n"
kill
quit

################ Scene 3: program replacement (TaskControlBlock::exec) ################
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch5-gdb.log
set logging overwrite off
set logging enabled on
target remote localhost:1234
break src/task/task.rs:171
commands
  silent
  printf "\n===== TaskControlBlock::exec: replace user program in place =====\n"
  bt 5
  set language rust
  print (*self).pid.0
  print entry_point
  print user_sp
  print trap_cx_ppn
  set language c
end
continue
kill
quit

################ Scene 4: exit and reclaim (sys_waitpid) ################
# start QEMU with stdin: printf 'ch5b_usertest\n' | qemu-system-riscv64 ... -s -S
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch5-gdb.log
set logging overwrite off
set logging enabled on
set $x = 0
target remote localhost:1234
break src/task/mod.rs:78
commands
  silent
  set $x = $x + 1
  if $x == 1
    printf "\n===== exit_current_and_run_next: process becomes Zombie =====\n"
    bt 5
    set language rust
    print (*task.ptr.pointer).data.pid.0
    print exit_code
    set language c
  end
  continue
end
break src/syscall/process.rs:103
commands
  silent
  printf "\n===== sys_waitpid reaps one Zombie child =====\n"
  bt 5
  set language rust
  print (*child.ptr.pointer).data.pid.0
  print (*child.ptr.pointer).data.inner.inner.value.value.exit_code
  set language c
end
continue
kill
quit
