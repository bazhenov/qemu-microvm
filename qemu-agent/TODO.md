## In progress

- [ ] add end to end test with QEMU

## Backlog

- [ ] proper exit code propagation to a host
- [ ] proper user initialization (no)
- [ ] add logging
- [ ] exiting failing with `pump_to_channel() = I/O error (os error 5)`
- [ ] add options to specify kernel and path to block device

# Jul 11

- [x] add ability to start random command

# Jul 3

- [x] isolate tmpdir with console tty when running VM
- [x] add option to dump boot logs to the output/file
- [x] add clap to server/client
- [x] add error reporting to all threads in server.rs

# Jun 30

- [x] top is failing when spamming <SPACE>
  - [x] add error reporting to all threads in client.rs
