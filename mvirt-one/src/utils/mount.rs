//! Filesystem mounting utilities for one.
//! Ported from pideisn.

use crate::error::{Error, Result};
use log::{error, info, warn};
use nix::mount::{MsFlags, mount};
use std::fs;
use std::path::Path;

struct MountPoint {
    source: &'static str,
    target: &'static str,
    fstype: &'static str,
    flags: MsFlags,
    data: Option<&'static str>,
}

const MOUNTS: &[MountPoint] = &[
    MountPoint {
        source: "proc",
        target: "/proc",
        fstype: "proc",
        flags: MsFlags::empty(),
        data: None,
    },
    MountPoint {
        source: "sysfs",
        target: "/sys",
        fstype: "sysfs",
        flags: MsFlags::empty(),
        data: None,
    },
    MountPoint {
        source: "devtmpfs",
        target: "/dev",
        fstype: "devtmpfs",
        flags: MsFlags::empty(),
        data: None,
    },
    MountPoint {
        source: "tmpfs",
        target: "/run",
        fstype: "tmpfs",
        flags: MsFlags::empty(),
        data: Some("mode=755"),
    },
    MountPoint {
        source: "tmpfs",
        target: "/tmp",
        fstype: "tmpfs",
        flags: MsFlags::empty(),
        data: Some("mode=1777"),
    },
    MountPoint {
        source: "cgroup2",
        target: "/sys/fs/cgroup",
        fstype: "cgroup2",
        flags: MsFlags::empty(),
        data: None,
    },
];

fn is_mounted(target: &str) -> bool {
    let Ok(mounts) = fs::read_to_string("/proc/mounts") else {
        return false;
    };
    mounts
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(target))
}

fn mount_one(mp: &MountPoint) -> Result<()> {
    if is_mounted(mp.target) {
        info!("{} already mounted", mp.target);
        return Ok(());
    }

    let target_path = Path::new(mp.target);
    if !target_path.exists() {
        fs::create_dir_all(target_path)?;
    }

    mount(
        Some(mp.source),
        mp.target,
        Some(mp.fstype),
        mp.flags,
        mp.data,
    )
    .map_err(|e| Error::Mount {
        target: mp.target.to_string(),
        source: e,
    })?;

    info!("Mounted {} on {}", mp.fstype, mp.target);
    Ok(())
}

/// Mount all required virtual filesystems.
/// Should only be called when running as PID 1.
pub fn mount_all() {
    for mp in MOUNTS {
        if let Err(e) = mount_one(mp) {
            error!("Failed to mount {}: {}", mp.target, e);
        }
    }

    // Create essential device nodes if devtmpfs didn't
    create_device_nodes();
}

fn create_device_nodes() {
    let nodes = [
        ("/dev/null", 1, 3),
        ("/dev/zero", 1, 5),
        ("/dev/random", 1, 8),
        ("/dev/urandom", 1, 9),
        ("/dev/tty", 5, 0),
        ("/dev/console", 5, 1),
    ];

    for (path, major, minor) in nodes {
        if Path::new(path).exists() {
            continue;
        }

        let dev = nix::sys::stat::makedev(major, minor);
        if let Err(e) = nix::sys::stat::mknod(
            path,
            nix::sys::stat::SFlag::S_IFCHR,
            nix::sys::stat::Mode::from_bits_truncate(0o666),
            dev,
        ) {
            warn!("Failed to create {}: {}", path, e);
        }
    }

    // Create /dev/pts for pseudo-terminals
    let pts_path = Path::new("/dev/pts");
    if !pts_path.exists() {
        if let Err(e) = fs::create_dir(pts_path) {
            warn!("Failed to create /dev/pts: {}", e);
        } else if let Err(e) = mount(
            Some("devpts"),
            "/dev/pts",
            Some("devpts"),
            MsFlags::empty(),
            Some("gid=5,mode=620"),
        ) {
            warn!("Failed to mount /dev/pts: {}", e);
        }
    }
}

/// Create required directories for container operations.
/// Call this for both PID 1 and local modes.
pub fn create_container_directories(base_path: &Path) -> Result<()> {
    let dirs = ["images", "pods", "layers"];

    for dir in dirs {
        let path = base_path.join(dir);
        if !path.exists() {
            fs::create_dir_all(&path)?;
            info!("Created directory: {}", path.display());
        }
    }

    Ok(())
}
