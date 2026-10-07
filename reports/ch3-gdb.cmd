set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch3-gdb.log
set logging overwrite on
set logging enabled on

printf "================================================================\n"
printf "rCore ch3 GDB trace: task startup, timer preemption, syscall\n"
printf "accounting, sys_yield suspend/resume and task exit\n"
printf "kernel: ch3 reference (MODE=debug), apps app_0..app_12 (BASE=2)\n"
printf "================================================================\n"

target remote localhost:1234

# ---------------------------------------------------------------------
# [0] startup: run_first_task -> TASK_MANAGER init -> task states
# ---------------------------------------------------------------------
printf "\n\n===== [0] startup: run_first_task -> TASK_MANAGER init =====\n"
tbreak os::task::run_first_task
continue
printf "-- hit os::task::run_first_task (before TASK_MANAGER first deref)\n"
bt 6
info registers ra sp

set $init_count = 0
break os::loader::init_app_cx
commands
silent
set $init_count = $init_count + 1
printf "[init_app_cx] call #%d, app_id = %d\n", $init_count, (int)$a1
continue
end

tbreak os::loader::get_num_app
continue
printf "-- get_num_app() during TASK_MANAGER init\n"
bt 4
finish
printf "-- get_num_app returned %d\n", (int)$a0

break os::task::TaskManager::run_first_task
continue
printf "-- hit TaskManager::run_first_task\n"
bt 6
delete os::loader::init_app_cx
printf "-- init_app_cx called %d times\n", $init_count

# ---------------------------------------------------------------------
# [1] first context switch -> __restore -> user mode
# ---------------------------------------------------------------------
printf "\n\n===== [1] __switch -> __restore -> user =====\n"
tbreak *__switch
continue
printf "-- first __switch entry\n"
bt 4
info registers a0 a1 ra sp
printf "-- a0 -> context to save (boot), a1 -> task 0 TaskContext\n"
x/14gx $a1
set $task0_cx = $a1

tbreak *__restore
continue
printf "-- first __restore; sp -> TrapContext (34 words)\n"
info registers sp
x/34gx $sp
printf "-- sepc = %#lx ; user sp x[2] = %#lx\n", ((unsigned long*)$sp)[33], ((unsigned long*)$sp)[2]

# ---------------------------------------------------------------------
# [2] timer interrupt -> set_next_trigger -> suspend -> __switch
# ---------------------------------------------------------------------
printf "\n\n===== [2] timer preemption =====\n"
tbreak trap_handler if $scause == 0x8000000000000005
continue
printf "-- trap_handler with SupervisorTimer\n"
bt 6
info registers scause sepc

tbreak os::timer::set_next_trigger
continue
printf "-- set_next_trigger\n"
bt 4

tbreak os::task::suspend_current_and_run_next
continue
printf "-- suspend_current_and_run_next (timer path)\n"
bt 6

tbreak os::task::TaskManager::find_next_task
continue
printf "-- find_next_task selects successor\n"
bt 5

tbreak *__switch
continue
printf "-- __switch after timer preemption\n"
info registers a0 a1 ra sp
x/14gx $a1
set $tcb_stride = (long)$a1 - $task0_cx
printf "-- TCB stride = %ld\n", $tcb_stride
printf "-- current task index = %ld, successor index = %ld\n", ((long)$a0 - $task0_cx)/$tcb_stride, ((long)$a1 - $task0_cx)/$tcb_stride

# ---------------------------------------------------------------------
# [3] syscall accounting: record before dispatch, sys_trace query
# ---------------------------------------------------------------------
printf "\n\n===== [3] syscall accounting =====\n"
tbreak os::task::record_current_syscall
continue
printf "-- first record_current_syscall (called before dispatch)\n"
bt 5
printf "-- syscall_id = %d\n", (int)$a0

printf "-- note: sys_trace (id=410) is not reached in this debug run: the\n"
printf "   debug user build makes ch3_sleep/ch3_trace panic on get_time()==0,\n"
printf "   so the sys_trace query path is analysed from source instead.\n"

# ---------------------------------------------------------------------
# [4] sys_yield -> suspend -> find_next -> __switch -> resume
# ---------------------------------------------------------------------
printf "\n\n===== [4] sys_yield (ch3b_yield0) =====\n"
tbreak os::syscall::process::sys_yield
continue
printf "-- hit sys_yield\n"
bt 5

tbreak os::task::suspend_current_and_run_next
continue
printf "-- suspend_current_and_run_next (yield path)\n"
bt 5

tbreak os::task::TaskManager::find_next_task
continue
printf "-- find_next_task selects successor\n"
bt 5

tbreak *__switch
continue
printf "-- __switch for yield\n"
info registers a0 a1 ra sp
printf "-- context to save (a0):\n"
x/14gx $a0
printf "-- context to restore (a1):\n"
x/14gx $a1
printf "-- current task index = %ld, successor index = %ld\n", ((long)$a0 - $task0_cx)/$tcb_stride, ((long)$a1 - $task0_cx)/$tcb_stride
set $sus_ra = $ra
set $sus_sp = $sp
printf "-- suspended task will resume at ra=%#lx with sp=%#lx\n", $sus_ra, $sus_sp

tbreak *$sus_ra if $sp == $sus_sp
continue
printf "-- suspended task resumed at its saved ra/sp\n"
bt 6
info registers ra sp

# ---------------------------------------------------------------------
# [5] sys_exit / exception -> exit_current_and_run_next
# ---------------------------------------------------------------------
printf "\n\n===== [5] task exit =====\n"
tbreak os::task::exit_current_and_run_next
continue
printf "-- exit_current_and_run_next\n"
bt 5

tbreak os::task::TaskManager::mark_current_exited
continue
printf "-- mark_current_exited\n"
bt 5

tbreak os::task::TaskManager::find_next_task
continue
printf "-- find_next_task after exit (Exited slot excluded)\n"
bt 5

tbreak *__switch
continue
printf "-- __switch after exit\n"
info registers a0 a1 ra sp
x/14gx $a1
printf "-- current task index = %ld, successor index = %ld\n", ((long)$a0 - $task0_cx)/$tcb_stride, ((long)$a1 - $task0_cx)/$tcb_stride

printf "\n\n===== end of trace =====\n"
