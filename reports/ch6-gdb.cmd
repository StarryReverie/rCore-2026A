# ch6 GDB session scripts (reference kernel, MODE=debug BASE=2)
#
# Build once:  cd os && make build MODE=debug BASE=2
#
# Scene A (boot-time program loading): no shell input needed.
#   qemu-system-riscv64 ... -s -S </dev/null
#   gdb -nx -batch -q target/riscv64gc-unknown-none-elf/debug/os -x SceneA
#
# Scene B (file operations): drive the user shell through qemu's stdio.
#   { sleep 8; printf 'ch6_file0\n'; sleep 8; printf 'ch6_file0\n'; \
#     sleep 8; printf 'ch6_file2\n'; sleep 120; } | qemu-system-riscv64 ... -s -S
#   gdb -nx -batch -q target/riscv64gc-unknown-none-elf/debug/os -x SceneB
#
# The two scenes use separate QEMU runs; each script overwrites/appends
# ../reports/ch6-gdb.log.  Run Scene A first (overwrite on), then Scene B.

################ Scene A: boot-time program loading ################
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch6-gdb.log
set logging overwrite on
set logging enabled on
target remote localhost:1234

printf "\n########## Scene A: boot-time program loading ##########\n"

tbreak os::fs::inode::open_file
continue
printf "\n===== [A1] os::fs::inode::open_file (initial program) =====\n"
bt 5
print flags
set language rust
print name
set language c

tbreak easy_fs::vfs::Inode::find
continue
printf "\n===== [A2] easy_fs::vfs::Inode::find =====\n"
bt 4
set language rust
print name
set language c

tbreak 'os::fs::inode::OSInode::read_all'
continue
printf "\n===== [A3] OSInode::read_all (load whole ELF) =====\n"
bt 6
set language rust
print *self
set language c

tbreak easy_fs::vfs::Inode::read_at
continue
printf "\n===== [A4] easy_fs::vfs::Inode::read_at =====\n"
bt 6
set language rust
print offset
set language c

kill
quit

################ Scene B: file operations (ch6_file0 x2, ch6_file2) ################
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch6-gdb.log
set logging overwrite off
set logging enabled on
target remote localhost:1234

printf "\n########## Scene B: file operations (ch6_file0 x2, ch6_file2) ##########\n"

set $create_n = 0
break easy_fs::vfs::Inode::create
commands
  silent
  set $create_n = $create_n + 1
  printf "\n===== [B] Inode::create #%d =====\n", $create_n
  bt 4
  set language rust
  print name
  set language c
  continue
end

set $clear_n = 0
break easy_fs::vfs::Inode::clear
commands
  silent
  set $clear_n = $clear_n + 1
  printf "\n===== [B] Inode::clear #%d (CREATE re-opens an existing name) =====\n", $clear_n
  bt 4
  continue
end

set $write_n = 0
break easy_fs::vfs::Inode::write_at
commands
  silent
  set $write_n = $write_n + 1
  printf "\n===== [B] Inode::write_at #%d =====\n", $write_n
  bt 4
  set language rust
  print offset
  set language c
  continue
end

set $grow_n = 0
break easy_fs::vfs::Inode::increase_size
commands
  silent
  set $grow_n = $grow_n + 1
  printf "\n===== [B] Inode::increase_size #%d =====\n", $grow_n
  bt 3
  set language rust
  print new_size
  set language c
  continue
end

set $link_n = 0
break easy_fs::vfs::Inode::link
commands
  silent
  set $link_n = $link_n + 1
  printf "\n===== [B] Inode::link #%d =====\n", $link_n
  bt 4
  set language rust
  print old_name
  print new_name
  set language c
  continue
end

set $stat_n = 0
break easy_fs::vfs::Inode::stat
commands
  silent
  set $stat_n = $stat_n + 1
  if $stat_n <= 3
    printf "\n===== [B] Inode::stat #%d =====\n", $stat_n
    bt 4
  end
  continue
end

set $unlink_n = 0
break easy_fs::vfs::Inode::unlink
commands
  silent
  set $unlink_n = $unlink_n + 1
  printf "\n===== [B] Inode::unlink #%d =====\n", $unlink_n
  bt 4
  set language rust
  print name
  set language c
  if $unlink_n >= 4
    printf "\n----- ch6_file2 removed all four names, ending session -----\n"
    kill
    quit
  end
  continue
end

continue
