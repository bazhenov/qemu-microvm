use std::{
    env, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Copy-on-write overlay over the read-only base image.
const OVERLAY: &str = "../rootfs-overlay.qcow2";

pub struct VmLaunchOpts {
    /// If true stdout/stderr of VM process will be linked to current
    /// terminal, so that boot logs will be visible.
    ///
    /// This only should be used for diagnostic, because it might break terminal working in VM
    pub dump_boot_log: bool,

    /// Path to a tty that will be linked to a serial device in a VM which is used for
    /// communicating with VM-server
    pub serial_path: PathBuf,
}

/// Launch the microVM under QEMU
///
///   1. remove the stale `./console` pty symlink,
///   2. create the qcow2 overlay disk on first run,
///   3. `exec` qemu-system-aarch64 with the full device set
///      (aarch64/HVF, virtio-serial console + data port, virtio-blk root,
///      user-mode net, RNG, a 9p share of the cwd, kernel + initrd).
///
/// Paths are relative to the current working directory.
pub fn launch_vm(opts: VmLaunchOpts) -> io::Result<()> {
    // create the overlay disk the first time, backed by rootfs.qcow2.
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

    let mut qemu_cmd = Command::new("qemu-system-aarch64");
    qemu_cmd
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
            &format!(
                "pty,signal=off,path={},id=console-hvc1",
                opts.serial_path.display()
            ),
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
        .stdin(Stdio::piped());

    if !opts.dump_boot_log {
        qemu_cmd.stderr(Stdio::piped()).stdout(Stdio::piped());
    }

    let mut qemu = qemu_cmd.spawn()?;
    let _ = qemu.stderr.take();
    let _ = qemu.stdin.take();
    qemu.wait_with_output()?;
    Ok(())
}
