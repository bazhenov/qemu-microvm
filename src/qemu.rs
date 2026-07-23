use std::{
    env, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const LINUX_KERNEL: &str = "./linux/arch/arm64/boot/Image";

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

    /// Root filesystem disk image attached as the first virtio-blk device
    /// (`/dev/vda`).
    ///
    /// The disk is booted read-write, the caller is responsible for preparing
    /// it (and cloning it beforehand if the original must stay intact, e.g.
    /// with APFS `clonefile`/`cp -c`). Format is inferred from the file
    /// extension: `.qcow2` — qcow2, anything else — raw.
    pub root_fs: PathBuf,

    /// If true, emulating mode is used, otherwise platform hypervisor is used
    pub emulate: bool,

    /// Command to run in the VM instead of the default login shell.
    ///
    /// Passed to the guest init through the kernel command line (everything
    /// after `--` is handed to init as its arguments). Ignored in recovery mode.
    pub command: Vec<String>,

    /// Additional disk images attached to the VM as virtio-blk devices.
    ///
    /// Disks appear in the guest after the root disk in the given order
    /// (`/dev/vdb`, `/dev/vdc`, ...). Format is inferred from the file
    /// extension: `.qcow2` — qcow2, anything else — raw.
    pub additional_disks: Vec<PathBuf>,
}

/// Launch the microVM under QEMU
///
///   1. remove the stale `./console` pty symlink,
///   2. `exec` qemu-system-aarch64 with the full device set
///      (aarch64/HVF, virtio-serial console + data port, virtio-blk root,
///      user-mode net, RNG, a 9p share of the cwd, kernel + initrd).
///
/// Paths are relative to the current working directory.
pub fn launch_vm(opts: VmLaunchOpts) -> io::Result<()> {
    if !opts.root_fs.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("root fs image not found: {}", opts.root_fs.display()),
        ));
    }

    // -virtfs local,path=$PWD,... — share the current working directory over 9p.
    let pwd = env::current_dir()?;
    let virtfs = format!(
        "local,path={},mount_tag=qemu,security_model=mapped",
        pwd.display()
    );

    let mut kernel_opts = vec![
        "console=hvc0".to_string(),
        "reboot=t".to_string(),
        "panic=-1".to_string(),
        "rdinit=/init".to_string(),
    ];
    if opts.recovery {
        kernel_opts.push("init_recovery".to_string());
    }

    if let Some(value) = format_init_args(&opts)? {
        kernel_opts.push("--".to_string());
        kernel_opts.push(value);
    }

    let kernel_opts = kernel_opts.join(" ");

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
        .args(["-m", "1G"]);

    // Additional disk drives (guest sees them as /dev/vdb, /dev/vdc, ...).
    for (idx, disk) in opts.additional_disks.iter().enumerate() {
        if !disk.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("disk image not found: {}", disk.display()),
            ));
        }
        let format = disk_format(disk);
        qemu_cmd.args([
            "-drive",
            &format!(
                "id=disk{idx},file={},format={format},if=none",
                disk.display()
            ),
            "-device",
            &format!("virtio-blk-device,drive=disk{idx}"),
        ]);
    }

    qemu_cmd
        // Root disk drive.
        .args([
            "-drive",
            &format!(
                "id=root,file={},format={},if=none",
                opts.root_fs.display(),
                disk_format(&opts.root_fs)
            ),
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
        // It's important to serial devices to be configured last (console and host pty).
        // We rely on device index (eg vport0p1) to connect VM to host, and devices enumerated by Linux/QEMU
        // in reverse order, so console options must be last or respective changes might be made to initrd script.
        //
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
                "pty,signal=off,path={},id=host-pty",
                opts.serial_path.display()
            ),
            "-device",
            "virtserialport,chardev=host-pty",
        ])
        // Realtime clock. PL031 linux driver is required.
        .args(["-rtc", "base=utc,clock=host"])
        // RNG support
        .args(["-device", "virtio-rng-pci"])
        // VirtIO FS share — path is computed at runtime, so pass it separately
        .args(["-virtfs", &virtfs])
        // Linux kernel settings
        .args(["-kernel", LINUX_KERNEL, "-initrd", "./target/initrd.gz"])
        .args(["-append", &kernel_opts])
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

fn disk_format(path: &Path) -> &'static str {
    match path.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("qcow2") => "qcow2",
        _ => "raw",
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
