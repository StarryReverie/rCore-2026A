set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch2-gdb.log
set logging overwrite on
set logging enabled on

printf "================================================================\n"
printf "rCore ch2 GDB trace: user ecall and application exceptions\n"
printf "kernel: ch2 reference (MODE=debug), apps app_0..app_6\n"
printf "================================================================\n"

target remote localhost:1234

# ---------------------------------------------------------------------
# Chain 1: rust_main -> batch::run_next_app -> AppManager::load_app
#          -> __restore -> user program
# ---------------------------------------------------------------------
printf "\n\n===== [1] rust_main -> run_next_app -> load_app -> __restore =====\n"
tbreak os::batch::AppManager::load_app
continue
printf "-- hit AppManager::load_app\n"
info args
bt 6

printf "\n-- continue to __restore (first app boot)\n"
tbreak *__restore
continue
printf "-- hit __restore, a0 = TrapContext pointer\n"
info registers a0
printf "-- TrapContext layout: x[0..31], sstatus, sepc (34 words)\n"
x/34gx $a0
printf "-- user sp is x[2] (third word):\n"
p/x ((unsigned long*)$a0)[2]

# ---------------------------------------------------------------------
# Chain 2: exception -> trap_handler -> run_next_app -> __restore
# ---------------------------------------------------------------------
printf "\n\n===== [2] exception -> trap_handler -> run_next_app =====\n"
printf "-- wait for first IllegalInstruction (scause == 2)\n"
tbreak trap_handler if $scause == 2
continue
printf "-- hit trap_handler with scause == 2\n"
bt 6
info registers scause stval
printf "-- saved sepc of the faulting instruction:\n"
p/x cx->sepc

# ---------------------------------------------------------------------
# Chain 3: ecall -> __alltraps -> trap_handler -> syscall -> sys_write
#          -> __restore
# ---------------------------------------------------------------------
printf "\n\n===== [3] ecall -> trap_handler -> syscall -> sys_write =====\n"
printf "-- wait for first write syscall (scause == 8 && a7 == 64)\n"
tbreak trap_handler if $scause == 8 && cx->x[17] == 64
continue
printf "-- hit trap_handler for write\n"
set $write_cx = cx
set $write_sepc = cx->sepc
printf "saved sepc (ecall) = %#lx\n", $write_sepc
printf "syscall_id (a7)  = %ld\n", cx->x[17]
printf "arg a0 (fd)      = %ld\n", cx->x[10]
printf "arg a1 (buf ptr) = %#lx\n", cx->x[11]
printf "arg a2 (len)     = %ld\n", cx->x[12]
bt 6

printf "\n-- continue into syscall()\n"
tbreak os::syscall::syscall
continue
printf "-- hit syscall(syscall_id, args)\n"
info args
bt 6
printf "\n-- finish syscall() then step the return-value writeback\n"
finish
next
printf "sepc delta after dispatch = %#lx\n", $write_cx->sepc - $write_sepc
printf "return value stored in a0 = %ld\n", $write_cx->x[10]

# ---------------------------------------------------------------------
# Chain 4: sys_exit -> run_next_app, then remaining batch
# ---------------------------------------------------------------------
printf "\n\n===== [4] sys_exit / remaining app loads / completion =====\n"
break os::batch::AppManager::load_app
commands
silent
printf "[load_app] app_id = %d\n", app_id
continue
end

break os::syscall::process::sys_exit
commands
silent
printf "[sys_exit] exit_code = %d\n", exit_code
continue
end

printf "-- continue to the end of the batch\n"
continue
