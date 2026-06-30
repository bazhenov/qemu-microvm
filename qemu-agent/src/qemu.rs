use std::env;
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;

/// Copy-on-write overlay over the read-only base image.
const OVERLAY: &str = "../rootfs-overlay.qcow2";
/// Symlink QEMU (re)creates pointing at the allocated pty for the data port.
pub const CONSOLE: &str = "../console";

/// Launch the microVM under QEMU — a Rust port of `run.sh`.
///
/// Mirrors the shell script step for step:
///   1. remove the stale `./console` pty symlink,
///   2. create the qcow2 overlay disk on first run,
///   3. `exec` qemu-system-aarch64 with the full device set
///      (aarch64/HVF, virtio-serial console + data port, virtio-blk root,
///      user-mode net, RNG, a 9p share of the cwd, kernel + initrd).
///
/// Paths are relative to the current working directory, exactly like the
/// script, so run it from the project root.
pub fn launch_vm() -> io::Result<()> {
    match fs::remove_file(CONSOLE) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    // run.sh: create the overlay disk the first time, backed by rootfs.qcow2.
    if !Path::new(OVERLAY).exists() {
        let status = Command::new("qemu-img")
            .args([
                "create",
                "-o",
                "backing_file=../rootfs.qcow2,backing_fmt=qcow2",
                "-f",
                "qcow2",
                OVERLAY,
            ])
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "qemu-img create failed: {status}"
            )));
        }
    }

    // -virtfs local,path=$PWD,... — share the current working directory over 9p.
    let pwd = env::current_dir()?;
    let virtfs = format!(
        "local,path={},mount_tag=qemu,security_model=mapped",
        pwd.display()
    );

    let mut qemu = Command::new("qemu-system-aarch64")
        .args([
            // General settings. Using Hypervisor.framework.
            "-accel",
            "hvf",
            "-cpu",
            "host",
            // General settings. Emulation.
            "-nodefaults",
            "-no-user-config",
            "-nographic",
            "-no-reboot",
            // CPU settings.
            "-M",
            "virt",
            "-smp",
            "cpus=1,sockets=1,cores=1,threads=1",
            "-m",
            "512M",
            // virtio-serial bus carrying the two ports below.
            "-device",
            "virtio-serial-device",
            // hvc0: console multiplexed onto stdio.
            "-chardev",
            "stdio,signal=off,id=console-hvc0",
            "-device",
            "virtconsole,chardev=console-hvc0",
            // Data port exposed to the host as the pty.
            "-chardev",
            &format!("pty,signal=off,path={},id=console-hvc1", CONSOLE),
            "-device",
            "virtserialport,chardev=console-hvc1",
            // Root disk drive.
            "-drive",
            &format!("id=root,file={},format=qcow2,if=none", OVERLAY),
            "-device",
            "virtio-blk-device,drive=root",
            // Network (user-mode networking).
            "-device",
            "virtio-net-device,netdev=net1",
            "-netdev",
            "user,id=net1",
            // Realtime clock. PL031 linux driver is required.
            "-rtc",
            "base=utc,clock=host",
            // RNG support.
            "-device",
            "virtio-rng-pci",
        ])
        // VirtIO FS share — path is computed at runtime, so pass it separately.
        .args(["-virtfs", &virtfs])
        .args([
            // Linux kernel settings.
            "-kernel",
            "../Image",
            "-initrd",
            "../initrd.gz",
            "-append",
            "console=hvc0 reboot=t rdinit=/init panic=-1",
        ])
        .stderr(Stdio::piped())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let _ = qemu.stderr.take();
    let _ = qemu.stdin.take();
    qemu.wait_with_output()?;
    Ok(())
}
