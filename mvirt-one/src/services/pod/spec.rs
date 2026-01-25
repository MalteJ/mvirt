//! OCI Runtime Spec generation for containers.

use crate::proto::ContainerSpec;
use log::info;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;

/// Generate OCI runtime spec (config.json) for a container.
pub async fn generate_oci_spec(
    container_spec: &ContainerSpec,
    rootfs_path: &str,
    bundle_path: &Path,
) -> Result<(), std::io::Error> {
    let spec = OciSpec::new(container_spec, rootfs_path);
    let spec_json = serde_json::to_string_pretty(&spec)?;

    fs::create_dir_all(bundle_path).await?;
    fs::write(bundle_path.join("config.json"), spec_json).await?;

    info!(
        "Generated OCI spec for container {} at {}",
        container_spec.id,
        bundle_path.display()
    );

    Ok(())
}

/// Minimal OCI Runtime Spec.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OciSpec {
    oci_version: String,
    root: Root,
    process: Process,
    hostname: String,
    mounts: Vec<Mount>,
    linux: Linux,
}

#[derive(Debug, Serialize, Deserialize)]
struct Root {
    path: String,
    readonly: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Process {
    terminal: bool,
    user: User,
    args: Vec<String>,
    env: Vec<String>,
    cwd: String,
    capabilities: Capabilities,
    no_new_privileges: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct User {
    uid: u32,
    gid: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    bounding: Vec<String>,
    effective: Vec<String>,
    inheritable: Vec<String>,
    permitted: Vec<String>,
    ambient: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Mount {
    destination: String,
    #[serde(rename = "type")]
    mount_type: String,
    source: String,
    options: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Linux {
    namespaces: Vec<Namespace>,
    resources: Resources,
    masked_paths: Vec<String>,
    readonly_paths: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Namespace {
    #[serde(rename = "type")]
    ns_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Resources {
    devices: Vec<DeviceRule>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeviceRule {
    allow: bool,
    access: String,
}

impl OciSpec {
    fn new(container_spec: &ContainerSpec, rootfs_path: &str) -> Self {
        let mut args = container_spec.command.clone();
        args.extend(container_spec.args.clone());
        if args.is_empty() {
            args = vec!["/bin/sh".to_string()];
        }

        let mut env = vec![
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
            "TERM=xterm".to_string(),
        ];
        env.extend(container_spec.env.clone());

        let cwd = if container_spec.working_dir.is_empty() {
            "/".to_string()
        } else {
            container_spec.working_dir.clone()
        };

        let capabilities = vec![
            "CAP_CHOWN".to_string(),
            "CAP_DAC_OVERRIDE".to_string(),
            "CAP_FSETID".to_string(),
            "CAP_FOWNER".to_string(),
            "CAP_MKNOD".to_string(),
            "CAP_NET_RAW".to_string(),
            "CAP_SETGID".to_string(),
            "CAP_SETUID".to_string(),
            "CAP_SETFCAP".to_string(),
            "CAP_SETPCAP".to_string(),
            "CAP_NET_BIND_SERVICE".to_string(),
            "CAP_SYS_CHROOT".to_string(),
            "CAP_KILL".to_string(),
            "CAP_AUDIT_WRITE".to_string(),
        ];

        OciSpec {
            oci_version: "1.0.0".to_string(),
            root: Root {
                path: rootfs_path.to_string(),
                readonly: false,
            },
            process: Process {
                terminal: false,
                user: User { uid: 0, gid: 0 },
                args,
                env,
                cwd,
                capabilities: Capabilities {
                    bounding: capabilities.clone(),
                    effective: capabilities.clone(),
                    inheritable: capabilities.clone(),
                    permitted: capabilities.clone(),
                    ambient: capabilities,
                },
                no_new_privileges: true,
            },
            hostname: container_spec.name.clone(),
            mounts: vec![
                Mount {
                    destination: "/proc".to_string(),
                    mount_type: "proc".to_string(),
                    source: "proc".to_string(),
                    options: vec![],
                },
                Mount {
                    destination: "/dev".to_string(),
                    mount_type: "tmpfs".to_string(),
                    source: "tmpfs".to_string(),
                    options: vec![
                        "nosuid".to_string(),
                        "strictatime".to_string(),
                        "mode=755".to_string(),
                        "size=65536k".to_string(),
                    ],
                },
                Mount {
                    destination: "/dev/pts".to_string(),
                    mount_type: "devpts".to_string(),
                    source: "devpts".to_string(),
                    options: vec![
                        "nosuid".to_string(),
                        "noexec".to_string(),
                        "newinstance".to_string(),
                        "ptmxmode=0666".to_string(),
                        "mode=0620".to_string(),
                    ],
                },
                Mount {
                    destination: "/dev/shm".to_string(),
                    mount_type: "tmpfs".to_string(),
                    source: "shm".to_string(),
                    options: vec![
                        "nosuid".to_string(),
                        "noexec".to_string(),
                        "nodev".to_string(),
                        "mode=1777".to_string(),
                        "size=65536k".to_string(),
                    ],
                },
                Mount {
                    destination: "/sys".to_string(),
                    mount_type: "sysfs".to_string(),
                    source: "sysfs".to_string(),
                    options: vec![
                        "nosuid".to_string(),
                        "noexec".to_string(),
                        "nodev".to_string(),
                        "ro".to_string(),
                    ],
                },
                Mount {
                    destination: "/sys/fs/cgroup".to_string(),
                    mount_type: "cgroup2".to_string(),
                    source: "cgroup".to_string(),
                    options: vec![
                        "nosuid".to_string(),
                        "noexec".to_string(),
                        "nodev".to_string(),
                        "relatime".to_string(),
                        "ro".to_string(),
                    ],
                },
            ],
            linux: Linux {
                namespaces: vec![
                    Namespace {
                        ns_type: "pid".to_string(),
                        path: None,
                    },
                    Namespace {
                        ns_type: "mount".to_string(),
                        path: None,
                    },
                    // Note: network, ipc, uts namespaces are shared within a pod
                ],
                resources: Resources {
                    devices: vec![DeviceRule {
                        allow: false,
                        access: "rwm".to_string(),
                    }],
                },
                masked_paths: vec![
                    "/proc/acpi".to_string(),
                    "/proc/kcore".to_string(),
                    "/proc/keys".to_string(),
                    "/proc/latency_stats".to_string(),
                    "/proc/timer_list".to_string(),
                    "/proc/timer_stats".to_string(),
                    "/proc/sched_debug".to_string(),
                    "/sys/firmware".to_string(),
                ],
                readonly_paths: vec![
                    "/proc/asound".to_string(),
                    "/proc/bus".to_string(),
                    "/proc/fs".to_string(),
                    "/proc/irq".to_string(),
                    "/proc/sys".to_string(),
                    "/proc/sysrq-trigger".to_string(),
                ],
            },
        }
    }
}
