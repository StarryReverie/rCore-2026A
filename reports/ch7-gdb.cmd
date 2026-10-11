# ch7 GDB session scripts (reference kernel, MODE=debug BASE=2)
#
# Build once:  cd os && make build MODE=debug BASE=2
#
# Scene A (pipe lifecycle + shell dup): drive the user shell through qemu stdio.
#   { sleep 12; printf 'ch7b_pipetest\n'; sleep 30; \
#     printf 'ch2b_hello_world > audit7\n'; sleep 30; } | \
#     qemu-system-riscv64 -m 128M -machine virt -nographic -bios ../bootloader/rustsbi-qemu.bin \
#       -device loader,file=target/riscv64gc-unknown-none-elf/debug/os.bin,addr=0x80200000 \
#       -drive file=../user/target/riscv64gc-unknown-none-elf/release/fs.img,if=none,format=raw,id=x0 \
#       -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -s -S
#   gdb -nx -batch -q target/riscv64gc-unknown-none-elf/debug/os -x SceneA
#
# Scene B (3000-byte transfer): feed 'ch7b_pipe_large_test' the same way.
#   gdb -nx -batch -q target/riscv64gc-unknown-none-elf/debug/os -x SceneB
#
# Scene A overwrites ../reports/ch7-gdb.log; Scene B appends to it.
# NixOS has no riscv64-unknown-elf-gdb; the multiarch `gdb` 17.2 is used.

################ Scene A: ch7b_pipetest + shell dup ################
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch7-gdb.log
set logging overwrite on
set logging enabled on
target remote localhost:1234

printf "\n########## Scene A: ch7b_pipetest + shell dup ##########\n"

tbreak os::syscall::fs::sys_pipe
continue
printf "\n===== [A1] sys_pipe entry (ch7b_pipetest) =====\n"
bt 4

tbreak os::fs::pipe::make_pipe
continue
printf "\n===== [A2] make_pipe =====\n"
bt 3

tbreak os::fs::pipe::PipeRingBuffer::set_write_end
continue
printf "\n===== [A3] PipeRingBuffer::set_write_end (register Weak) =====\n"
bt 3
set language rust
print *write_end
set language c

set $wb = 0
break os::fs::pipe::PipeRingBuffer::write_byte
commands
  silent
  set $wb = $wb + 1
  if $wb <= 2 || $wb == 13
    printf "\n===== [A4] write_byte #%d byte=%d =====\n", $wb, byte
    bt 3
    printf "head=%d tail=%d status=%d\n", self->head, self->tail, self->status
  end
  continue
end

set $rb = 0
break os::fs::pipe::PipeRingBuffer::read_byte
commands
  silent
  set $rb = $rb + 1
  if $rb <= 2 || $rb == 13
    printf "\n===== [A5] read_byte #%d =====\n", $rb
    bt 3
    printf "head=%d tail=%d status=%d\n", self->head, self->tail, self->status
  end
  continue
end

set $ew = 0
break os::fs::pipe::PipeRingBuffer::all_write_ends_closed
commands
  silent
  set $ew = $ew + 1
  if $ew <= 5
    printf "\n===== [A6] all_write_ends_closed #%d =====\n", $ew
    bt 4
  end
  continue
end

tbreak os::syscall::fs::sys_dup
continue
printf "\n===== [A7] sys_dup (shell redirection) =====\n"
bt 4
print fd

printf "\n########## Scene A done ##########\n"
kill
quit

################ Scene B: ch7b_pipe_large_test (3000-byte transfer) ################
set pagination off
set confirm off
set architecture riscv:rv64
set language c
set logging file ../reports/ch7-gdb.log
set logging overwrite off
set logging enabled on
target remote localhost:1234

printf "\n########## Scene B: ch7b_pipe_large_test (3000-byte transfer) ##########\n"

break 'os::fs::pipe::{impl#2}::write'
commands
  silent
  printf "\n===== [B1] Pipe::write entry =====\n"
  bt 3
  continue
end

break 'os::fs::pipe::{impl#2}::read'
commands
  silent
  printf "\n===== [B2] Pipe::read entry =====\n"
  bt 3
  continue
end

set $wb = 0
break os::fs::pipe::PipeRingBuffer::write_byte
commands
  silent
  set $wb = $wb + 1
  if $wb <= 2 || $wb % 32 == 1
    printf "\n===== [B3] write_byte #%d =====\n", $wb
    bt 3
    printf "head=%d tail=%d status=%d\n", self->head, self->tail, self->status
  end
  if $wb >= 96
    printf "\n----- observed 3 full 32-byte batches (96 bytes), ending Scene B -----\n"
    kill
  else
    continue
  end
end

continue
quit
