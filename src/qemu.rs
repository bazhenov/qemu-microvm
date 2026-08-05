use std::{
    io::{self, BufRead, BufReader},
    net::TcpListener,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
};
use tempdir::TempDir;

/// Default Linux kernel image path (see `Makefile` for how it is built).
pub const DEFAULT_KERNEL: &str = "./linux/arch/arm64/boot/Image";

/// Default initrd image path (built by `make ./target/initrd.gz`).
pub const DEFAULT_INITRD: &str = "./target/initrd.gz";

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

    /// Number of cores
    pub cores: u16,

    /// The amount of memory in a VM in MB
    pub memory_megs: u32,

    /// Root filesystem disk image attached as the first virtio-blk device
    /// (`/dev/vda`).
    ///
    /// The disk is booted read-write, the caller is responsible for preparing
    /// it (and cloning it beforehand if the original must stay intact, e.g.
    /// with APFS `clonefile`/`cp -c`). Format is inferred from the file
    /// extension: `.qcow2` — qcow2, anything else — raw.
    pub root_fs: PathBuf,

    /// Linux kernel image booted in the VM
    pub kernel: PathBuf,

    /// Initrd image handed to the kernel
    pub initrd: PathBuf,

    /// If true, emulating mode is used, otherwise platform hypervisor is used
    pub emulate: bool,

    /// Command to run in the VM instead of the default login shell.
    ///
    /// Passed to the guest init through the kernel command line (everything
    /// after `--` is handed to init as its arguments).
    pub command: Vec<String>,

    /// Additional disk images attached to the VM as virtio-blk devices.
    ///
    /// Disks appear in the guest after the root disk in the given order
    /// (`/dev/vdb`, `/dev/vdc`, ...). Format is inferred from the file
    /// extension: `.qcow2` — qcow2, anything else — raw.
    pub additional_disks: Vec<PathBuf>,

    /// Localhost TCP port the guest NIC backend connects to. A gvproxy
    /// instance must already be listening on it (see [`start_gvproxy`]).
    pub net_port: u16,
}

/// Launch the microVM under QEMU and wait for it to exit, returning its exit
/// status
///
/// Paths are relative to the current working directory.
pub fn run_vm(opts: VmLaunchOpts) -> io::Result<ExitStatus> {
    let mut kernel_opts = vec![
        "console=hvc0".to_string(),
        "reboot=t".to_string(),
        "panic=-1".to_string(),
        "rdinit=/init".to_string(),
    ];
    if opts.recovery {
        kernel_opts.push("init_recovery=1".to_string());
    }

    if let Some(value) = format_init_args(&opts.command)? {
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
        .args([
            "-M",
            "virt",
            "-smp",
            &format!("cpus=1,sockets=1,cores={},threads=1", opts.cores),
        ])
        // Memory settings
        .args(["-m", &format!("{}M", opts.memory_megs)]);

    // Additional disk drives (guest sees them as /dev/vdb, /dev/vdc, ...).
    for (idx, disk) in opts.additional_disks.iter().enumerate() {
        ensure_exists(disk, "disk image")?;
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

    // Check before spawning QEMU: it creates the serial pty (and its symlink)
    // before opening the drives, so a boot doomed by a missing root fs would
    // still briefly expose a pty that `run` mistakes for a running VM.
    ensure_exists(&opts.root_fs, "root fs image")?;
    ensure_exists(&opts.kernel, "kernel image")?;
    ensure_exists(&opts.initrd, "initrd image")?;

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
        // Network: gvproxy is already listening on this port.
        .args([
            "-device",
            "virtio-net-device,netdev=net1",
            "-netdev",
            &format!("socket,id=net1,connect=127.0.0.1:{}", opts.net_port),
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
        // Linux kernel settings
        .arg("-kernel")
        .arg(&opts.kernel)
        .arg("-initrd")
        .arg(&opts.initrd)
        .args(["-append", &kernel_opts])
        // The guest console rides the serial pty, QEMU's own stdin is unused
        .stdin(Stdio::null());

    if !opts.dump_boot_log {
        qemu_cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }

    qemu_cmd.spawn()?.wait()
}

/// [`io::ErrorKind::NotFound`] with a readable message unless `path` exists
fn ensure_exists(path: &Path, what: &str) -> io::Result<()> {
    if path.exists() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{what} not found: {}", path.display()),
        ))
    }
}

/// Spawn gvproxy on a random localhost TCP port and wait until it is
/// ready to accept the QEMU connection, returning the process and the port
/// (for [`VmLaunchOpts::net_port`]).
///
/// Readiness is reported by gvproxy itself: it connects to a unix socket we
/// listen on (`-notification`) and sends a `{"notification_type":"ready"}`
/// json message once its qemu endpoint is accepting connections.
///
/// gvproxy exits on its own as soon as the QEMU connection closes; still the
/// caller should kill/reap the returned child once the VM is done — that
/// covers the paths where QEMU never connected (failed to spawn or died
/// before opening the netdev), and makes reaping prompt otherwise.
///
/// With `debug` gvproxy logs verbosely to the inherited stderr, otherwise its
/// output is discarded.
pub fn start_gvproxy(debug: bool) -> io::Result<(Child, u16)> {
    // Let the OS pick a free port. Technically racy (the port is released
    // before gvproxy binds it), but if the port gets stolen gvproxy just
    // fails to bind and the wait below reports it.
    let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();

    // The socket (and its directory) lives only for the duration of the
    // handshake; gvproxy keeps writing notifications to the connected end,
    // but nobody is listening anymore and gvproxy is fine with that.
    let notify_dir = TempDir::new("gvproxy")?;
    let notify_path = notify_dir.path().join("notify.sock");
    let notify = UnixListener::bind(&notify_path)?;

    let mut cmd = Command::new("gvproxy");
    cmd.arg("-listen-qemu")
        .arg(format!("tcp://127.0.0.1:{port}"))
        .arg("-notification")
        .arg(format!("unix://{}", notify_path.display()))
        // By default gvproxy forwards host port 2222 to the guest's SSH port;
        // that fixed port would allow only one VM at a time, so disable it.
        .args(["-ssh-port", "-1"])
        .stdin(Stdio::null());
    if debug {
        cmd.arg("-debug");
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let mut child = cmd.spawn().map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("cannot spawn gvproxy (is it on PATH?): {e}"),
        )
    })?;

    match wait_until_gvproxy_report_ready(notify) {
        Ok(()) => Ok((child, port)),
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(e)
        }
    }
}

fn wait_until_gvproxy_report_ready(notify: UnixListener) -> io::Result<()> {
    let (stream, _) = notify.accept()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::other("gvproxy closed the notification socket"));
        }
        // The message we are waiting for is `{"notification_type":"ready"}`
        if line.contains("\"ready\"") {
            return Ok(());
        }
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
fn format_init_args(command: &[String]) -> io::Result<Option<String>> {
    if command.is_empty() {
        Ok(None)
    } else {
        let mut args_line = String::new();
        for (idx, arg) in command.iter().enumerate() {
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
