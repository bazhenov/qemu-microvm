use std::{
    env, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Copy-on-write overlay over the read-only base image.
const OVERLAY: &str = "rootfs-overlay.qcow2";

pub struct VmLaunchOpts {
    /// If true stdout/stderr of VM process will be linked to current
    /// terminal, so that boot logs will be visible.
    ///
    /// This only should be used for diagnostic, because it might break terminal working in VM
    pub dump_boot_log: bool,

    /// Path to a tty that will be linked to a serial device in a VM which is used for
    /// communicating with VM-server
    pub serial_path: PathBuf,

    /// Start init in recovery mode
    pub recovery: bool,

    /// If true, emulating mode is used, otherwise platform hypervisor is used
    pub emulate: bool,

    /// Command to run in the VM instead of the default login shell.
    ///
    /// Passed to the guest init through the kernel command line (everything
    /// after `--` is handed to init as its arguments). Ignored in recovery mode.
    pub command: Vec<String>,
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

    let mut append = format!(
        "console=hvc0 reboot=t panic=-1 {} rdinit=/init",
        if opts.recovery { "init_recovery" } else { "" }
    );
    if let Some(value) = format_init_args(&opts)? {
        append.push_str(" -- ");
        append.push_str(&value);
    }

    let mut qemu_cmd = Command::new("qemu-system-aarch64");

    if opts.emulate {
        qemu_cmd.args(["-cpu", "cortex-a76"]);
    } else {
        // General settings. Using Hypervisor.framework.
        qemu_cmd.args(["-accel", "hvf", "-cpu", "host"]);
    }

    qemu_cmd
        // General settings.
        .args(["-nodefaults", "-no-user-config", "-nographic", "-no-reboot"])
        // CPU settings
        .args(["-M", "virt", "-smp", "cpus=1,sockets=1,cores=1,threads=1"])
        // Memory settings
        .args(["-m", "1G"])
        // virtio-serial bus carrying the two ports below
        .args(["-device", "virtio-serial-device"])
        // hvc0: console multiplexed onto stdio.
        .args([
            "-chardev",
            "stdio,signal=off,id=console-hvc0",
            "-device",
            "virtconsole,chardev=console-hvc0",
        ])
        // Data port exposed to the host as the pty.
        .args([
            "-chardev",
            &format!(
                "pty,signal=off,path={},id=console-hvc1",
                opts.serial_path.display()
            ),
            "-device",
            "virtserialport,chardev=console-hvc1",
        ])
        // Root disk drive.
        .args([
            "-drive",
            &format!("id=root,file={},format=qcow2,if=none", OVERLAY),
            "-device",
            "virtio-blk-device,drive=root",
        ])
        // Network (user-mode networking).
        .args([
            "-device",
            "virtio-net-device,netdev=net1",
            "-netdev",
            "user,id=net1",
        ])
        // Realtime clock. PL031 linux driver is required.
        .args(["-rtc", "base=utc,clock=host"])
        // RNG support
        .args(["-device", "virtio-rng-pci"])
        // VirtIO FS share — path is computed at runtime, so pass it separately
        .args(["-virtfs", &virtfs])
        // Linux kernel settings
        .args(["-kernel", "../Image", "-initrd", "../initrd.gz"])
        .args(["-append", &append])
        .stdin(Stdio::piped());

    if !opts.dump_boot_log {
        qemu_cmd.stderr(Stdio::piped()).stdout(Stdio::piped());
    }

    let mut qemu = qemu_cmd.spawn()?;
    let _ = qemu.stderr.take();
    let _ = qemu.stdin.take();
    let output = qemu.wait_with_output()?;
    if !output.status.success() {
        Err(io::Error::other(
            "VM failed, use --boot-log to inspect details",
        ))
    } else {
        Ok(())
    }
}

/// init arguments are by convention passed after `--` in the kernel args line
///
/// This method formats this arguments line. Result does not contains `--` separate itself
fn format_init_args(opts: &VmLaunchOpts) -> Result<Option<String>, io::Error> {
    if opts.command.is_empty() {
        Ok(None)
    } else {
        let mut args_line = String::new();
        for (idx, arg) in opts.command.iter().enumerate() {
            if arg.contains('"') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("double quotes are not supported in command arguments: {arg}"),
                ));
            }
            if idx > 0 {
                args_line.push(' ');
            }
            // The kernel tokenizer splits on spaces unless the value is quoted
            if arg.contains(' ') {
                args_line.push_str(&format!("\"{arg}\""));
            } else {
                args_line.push_str(arg);
            }
        }
        Ok(Some(args_line))
    }
}
