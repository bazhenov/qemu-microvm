## In progress

## Backlog

- [ ] proper user initialization (no)
- [ ] add logging
- [ ] exiting failing with `pump_to_channel() = I/O error (os error 5)`
- [ ] add options to specify kernel and path to block device

# Jul 31

- [x] tests on redirecting to stderr
- [x] provide a way to build rootfs

# Jul 24

- [x] using stdin

# Jul 16

- [x] add end to end test with QEMU

# Jul 13

- [x] proper exit code propagation to a host

# Jul 11

- [x] add ability to start random command

# Jul 3

- [x] isolate tmpdir with console tty when running VM
- [x] add option to dump boot logs to the output/file
- [x] add clap to server/vmctl
- [x] add error reporting to all threads in server.rs

# Jun 30

- [x] top is failing when spamming <SPACE>
  - [x] add error reporting to all threads in vmctl.rs
