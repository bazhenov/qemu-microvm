use nix::unistd::execv;
use std::{
    ffi::CStr,
    fs::{self, create_dir},
    process::exit,
};
use sys_mount::{Mount, MountFlags, SupportedFilesystems, Unmount, UnmountFlags};

fn main() {
    let proc = Mount::builder()
        .fstype("devtmpfs")
        .mount("devtmpfs", "/dev");
    if let Err(e) = proc {
        eprintln!("Failed to mount /proc: {e}");
    }

    create_dir("/sys").expect("Unable to create /sys");
    let proc = Mount::builder().fstype("sysfs").mount("sysfs", "/sys");
    if let Err(e) = proc {
        eprintln!("Failed to mount /sys: {e}");
    }

    create_dir("/proc").expect("Unable to create /proc");
    let proc = Mount::builder().fstype("proc").mount("proc", "/proc");
    if let Err(e) = proc {
        eprintln!("Failed to mount /proc: {e}");
    }

    create_dir("/mnt").expect("Unable to create /mnt");
    let proc = Mount::builder().fstype("ext4").mount("/dev/vda", "/mnt");
    if let Err(e) = proc {
        eprintln!("Failed to mount: {e}");
    }

    println!("Exist: {:?}", fs::exists("/mnt/sbin/init"));

    unsafe { execv::<&'static CStr>(c"/mnt/sbin/init", &[]) };
}
