//! OCI Runtime Spec generation for containers.

use crate::proto::ContainerSpec;
use crate::services::image::ImageConfig;
use log::info;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;

/// Generate OCI runtime spec (config.json) for a container.
pub async fn generate_oci_spec(
    container_spec: &ContainerSpec,
    rootfs_path: &str,
    bundle_path: &Path,
    image_config: &ImageConfig,
) -> Result<(), std::io::Error> {
    info!(
        "OCI spec: image entrypoint={:?}, cmd={:?}",
        image_config.entrypoint, image_config.cmd
    );
    let spec = OciSpec::new(container_spec, rootfs_path, image_config);
    let spec_json = serde_json::to_string_pretty(&spec)?;

    // Log the process.args from the JSON for debugging
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&spec_json)
        && let Some(args) = v.get("process").and_then(|p| p.get("args"))
    {
        info!("OCI spec process.args: {}", args);
    }

    fs::create_dir_all(bundle_path).await?;
    fs::write(bundle_path.join("config.json"), &spec_json).await?;

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
struct Process {
    terminal: bool,
    user: User,
    args: Vec<String>,
    env: Vec<String>,
    cwd: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct User {
    uid: u32,
    gid: u32,
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
}

#[derive(Debug, Serialize, Deserialize)]
struct Namespace {
    #[serde(rename = "type")]
    ns_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

impl OciSpec {
    fn new(container_spec: &ContainerSpec, rootfs_path: &str, image_config: &ImageConfig) -> Self {
        // Determine process args following Kubernetes/CRI semantics:
        // - command overrides ENTRYPOINT
        // - args overrides CMD
        // - If only args is set, use image ENTRYPOINT + container args
        let args = match (
            container_spec.command.is_empty(),
            container_spec.args.is_empty(),
        ) {
            // Both empty: use image ENTRYPOINT + CMD
            (true, true) => {
                let mut args = image_config.entrypoint.clone();
                args.extend(image_config.cmd.clone());
                if args.is_empty() {
                    vec!["/bin/sh".to_string()]
                } else {
                    args
                }
            }
            // Only command set: use command (ignore image CMD)
            (false, true) => container_spec.command.clone(),
            // Only args set: use image ENTRYPOINT + container args
            (true, false) => {
                let mut args = image_config.entrypoint.clone();
                args.extend(container_spec.args.clone());
                if args.is_empty() {
                    // Fallback if image has no entrypoint
                    container_spec.args.clone()
                } else {
                    args
                }
            }
            // Both set: use command + args
            (false, false) => {
                let mut args = container_spec.command.clone();
                args.extend(container_spec.args.clone());
                args
            }
        };

        // Start with standard PATH, add image env, then container env
        let mut env = vec![
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
            "TERM=xterm".to_string(),
        ];
        env.extend(image_config.env.clone());
        env.extend(container_spec.env.clone());

        // Use container working_dir, or image working_dir, or default to /
        let cwd = if !container_spec.working_dir.is_empty() {
            container_spec.working_dir.clone()
        } else if !image_config.working_dir.is_empty() {
            image_config.working_dir.clone()
        } else {
            "/".to_string()
        };

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
                // Writable /tmp for applications that need it
                Mount {
                    destination: "/tmp".to_string(),
                    mount_type: "tmpfs".to_string(),
                    source: "tmpfs".to_string(),
                    options: vec![
                        "nosuid".to_string(),
                        "nodev".to_string(),
                        "mode=1777".to_string(),
                    ],
                },
                // Writable /run for PID files and sockets
                Mount {
                    destination: "/run".to_string(),
                    mount_type: "tmpfs".to_string(),
                    source: "tmpfs".to_string(),
                    options: vec![
                        "nosuid".to_string(),
                        "nodev".to_string(),
                        "mode=755".to_string(),
                    ],
                },
            ],
            linux: Linux {
                // Create only pid and mount namespaces.
                // Network namespace is NOT listed, so the container inherits
                // the parent's network namespace (per OCI runtime spec).
                // IPC and UTS namespaces are not used since they require
                // additional kernel support.
                namespaces: vec![
                    Namespace {
                        ns_type: "pid".to_string(),
                        path: None,
                    },
                    Namespace {
                        ns_type: "mount".to_string(),
                        path: None,
                    },
                ],
            },
        }
    }
}
