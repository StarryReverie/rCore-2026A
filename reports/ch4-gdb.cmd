set pagination off
set confirm off
set architecture riscv:rv64
set language rust

set logging file ../reports/ch4-gdb.log
set logging overwrite on
set logging enabled on

target remote localhost:1234

# ===============================================================
# Stop when every application has finished.
# ===============================================================
break src/task/mod.rs:173
commands
  silent
  printf "\n===== end: all applications completed =====\n"
  bt 4
  printf "===== end of gdb session =====\n"
  quit
end

# ===============================================================
# Chain 1: mm::init -> KERNEL_SPACE (new_kernel -> MapArea::map ->
#          PageTable::map) -> activate -> remap_test
# ===============================================================
break os::mm::memory_set::MemorySet::new_kernel
commands
  silent
  printf "\n===== [1] MemorySet::new_kernel (KERNEL_SPACE lazy init) =====\n"
  bt 4
  continue
end

set $n_map = 0
break src/mm/page_table.rs:144
commands
  silent
  set $n_map = $n_map + 1
  if $n_map <= 4
    printf "\n----- [1] PageTable::map #%d -----\n", $n_map
    info args
    p/x vpn.0
    p/x ppn.0
    p/x flags.bits
    bt 6
  end
  if $n_map == 4
    disable $_hit_bpnum
  end
  continue
end

break os::mm::memory_set::MemorySet::activate
commands
  silent
  printf "\n===== [1] MemorySet::activate =====\n"
  bt 3
  p/x $satp
  continue
end

break src/mm/memory_set.rs:256
commands
  silent
  printf "----- [1] satp after activate -----\n"
  p/x $satp
  continue
end

break os::mm::memory_set::remap_test
commands
  silent
  printf "\n===== [1] remap_test (kernel segment permissions) =====\n"
  bt 3
  p/x $satp
  continue
end

# ===============================================================
# Chain 2: TaskControlBlock::new -> MemorySet::from_elf
# ===============================================================
set $n_elf = 0
break src/mm/memory_set.rs:165
commands
  silent
  set $n_elf = $n_elf + 1
  if $n_elf <= 2
    printf "\n===== [2] MemorySet::from_elf #%d =====\n", $n_elf
    info args
    bt 5
  end
  if $n_elf == 2
    disable $_hit_bpnum
  end
  continue
end

set $n_stack = 0
break src/mm/memory_set.rs:210
commands
  silent
  set $n_stack = $n_stack + 1
  if $n_stack <= 2
    printf "\n----- [2] from_elf user stack #%d -----\n", $n_stack
    p/x user_stack_bottom
    p/x user_stack_top
    p/x max_end_vpn.0
  end
  if $n_stack == 2
    disable $_hit_bpnum
  end
  continue
end

# ===============================================================
# Chain 3: sys_write -> translated_byte_buffer -> from_token/translate
# ===============================================================
set $n_wr = 0
break src/syscall/fs.rs:10
commands
  silent
  set $n_wr = $n_wr + 1
  if $n_wr <= 3
    printf "\n===== [3] sys_write #%d =====\n", $n_wr
    info args
    bt 3
  end
  if $n_wr == 3
    disable $_hit_bpnum
  end
  continue
end

set $n_tbb = 0
break src/mm/page_table.rs:184
commands
  silent
  set $n_tbb = $n_tbb + 1
  if $n_tbb <= 3
    printf "\n----- [3] translated_byte_buffer #%d -----\n", $n_tbb
    info args
    bt 5
  end
  if $n_tbb == 3
    disable $_hit_bpnum
  end
  continue
end

# ===============================================================
# Chain 4: sys_mmap -> insert_framed_area -> MapArea::map -> PageTable::map
# ===============================================================
set $n_mmap = 0
break src/syscall/process.rs:100
commands
  silent
  set $n_mmap = $n_mmap + 1
  if $n_mmap <= 4
    printf "\n===== [4] sys_mmap #%d =====\n", $n_mmap
    info args
    bt 3
  end
  if $n_mmap == 4
    disable $_hit_bpnum
  end
  continue
end

set $n_ins = 0
break src/mm/memory_set.rs:61
commands
  silent
  set $n_ins = $n_ins + 1
  if $n_ins <= 6
    printf "\n----- [4] insert_framed_area #%d -----\n", $n_ins
    info args
    bt 4
  end
  if $n_ins == 6
    disable $_hit_bpnum
  end
  continue
end

# ===============================================================
# Chain 5: sys_munmap -> remove_framed_area -> MapArea::unmap
# ===============================================================
break src/syscall/process.rs:129
commands
  silent
  printf "\n===== [5] sys_munmap =====\n"
  info args
  bt 3
  continue
end

break src/mm/memory_set.rs:284
commands
  silent
  printf "\n----- [5] remove_framed_area -----\n"
  info args
  bt 4
  continue
end

# ===============================================================
# Chain 6: sys_sbrk -> change_program_brk -> append_to / shrink_to
# ===============================================================
break src/syscall/process.rs:145
commands
  silent
  printf "\n===== [6] sys_sbrk =====\n"
  info args
  bt 3
  continue
end

break src/mm/memory_set.rs:311
commands
  silent
  printf "\n----- [6] MemorySet::append_to -----\n"
  info args
  bt 4
  continue
end

break src/mm/memory_set.rs:296
commands
  silent
  printf "\n----- [6] MemorySet::shrink_to -----\n"
  info args
  bt 4
  continue
end

# ===============================================================
# Chain 7: sys_trace -> PageTable::translate_user
# ===============================================================
set $n_tr = 0
break src/syscall/process.rs:70
commands
  silent
  set $n_tr = $n_tr + 1
  if $n_tr <= 8
    printf "\n===== [7] sys_trace #%d =====\n", $n_tr
    info args
    bt 3
  end
  if $n_tr == 8
    disable $_hit_bpnum
  end
  continue
end

set $n_tu = 0
break src/mm/page_table.rs:163
commands
  silent
  set $n_tu = $n_tu + 1
  if $n_tu <= 8
    printf "\n----- [7] translate_user #%d -----\n", $n_tu
    info args
    bt 5
  end
  if $n_tu == 8
    disable $_hit_bpnum
  end
  continue
end

continue
