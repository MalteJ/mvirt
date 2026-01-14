use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::transport::Channel;

use crate::proto::vm_service_client::VmServiceClient;
use crate::proto::*;
use crate::tui::types::{
    Action, ActionResult, CreateVmParams, DiskSourceType, SshKeySource, SshKeysConfig,
    StorageState, UserDataMode,
};
use crate::zfs_proto::zfs_service_client::ZfsServiceClient;
use crate::zfs_proto::*;

async fn generate_user_data(
    params: &CreateVmParams,
    result_tx: &mpsc::UnboundedSender<ActionResult>,
) -> Option<String> {
    match params.user_data_mode {
        UserDataMode::None => None,
        UserDataMode::File => {
            if let Some(path) = &params.user_data_file {
                match tokio::fs::read_to_string(path).await {
                    Ok(content) => Some(content),
                    Err(e) => {
                        let _ = result_tx.send(ActionResult::Created(Err(format!(
                            "Failed to read user-data file: {}",
                            e
                        ))));
                        None
                    }
                }
            } else {
                None
            }
        }
        UserDataMode::SshKeys => {
            if let Some(ref cfg) = params.ssh_keys_config {
                let ssh_keys = fetch_ssh_keys(cfg, result_tx).await?;
                Some(generate_cloud_init_yaml(cfg, &ssh_keys))
            } else {
                None
            }
        }
    }
}

async fn fetch_ssh_keys(
    cfg: &SshKeysConfig,
    result_tx: &mpsc::UnboundedSender<ActionResult>,
) -> Option<Vec<String>> {
    match cfg.source {
        SshKeySource::GitHub => {
            let url = format!("https://github.com/{}.keys", cfg.github_user);
            match reqwest::get(&url).await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        match resp.text().await {
                            Ok(keys) => Some(
                                keys.lines()
                                    .filter(|l| !l.is_empty())
                                    .map(|s| s.to_string())
                                    .collect(),
                            ),
                            Err(e) => {
                                let _ = result_tx.send(ActionResult::Created(Err(format!(
                                    "Failed to read GitHub keys: {}",
                                    e
                                ))));
                                None
                            }
                        }
                    } else {
                        let _ = result_tx.send(ActionResult::Created(Err(format!(
                            "Failed to fetch GitHub keys: HTTP {}",
                            resp.status()
                        ))));
                        None
                    }
                }
                Err(e) => {
                    let _ = result_tx.send(ActionResult::Created(Err(format!(
                        "Failed to fetch GitHub keys: {}",
                        e
                    ))));
                    None
                }
            }
        }
        SshKeySource::Local => match tokio::fs::read_to_string(&cfg.local_path).await {
            Ok(content) => Some(
                content
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(|s| s.to_string())
                    .collect(),
            ),
            Err(e) => {
                let _ = result_tx.send(ActionResult::Created(Err(format!(
                    "Failed to read SSH key file: {}",
                    e
                ))));
                None
            }
        },
    }
}

fn generate_cloud_init_yaml(cfg: &SshKeysConfig, ssh_keys: &[String]) -> String {
    let keys_yaml = ssh_keys
        .iter()
        .map(|k| format!("      - {}", k))
        .collect::<Vec<_>>()
        .join("\n");

    let password_yaml = if !cfg.root_password.is_empty() {
        format!(
            "\n    lock_passwd: false\n    plain_text_passwd: {}",
            cfg.root_password
        )
    } else {
        String::new()
    };

    let chpasswd_yaml = if !cfg.root_password.is_empty() {
        "\nchpasswd:\n  expire: false\nssh_pwauth: true"
    } else {
        ""
    };

    format!(
        "#cloud-config\nusers:\n  - name: {}\n    sudo: ALL=(ALL) NOPASSWD:ALL\n    shell: /bin/bash{}\n    ssh_authorized_keys:\n{}{}",
        cfg.username, password_yaml, keys_yaml, chpasswd_yaml
    )
}

async fn refresh_storage(
    zfs_client: &mut ZfsServiceClient<Channel>,
) -> Result<StorageState, String> {
    // Fetch pool stats
    let pool = match zfs_client.get_pool_stats(GetPoolStatsRequest {}).await {
        Ok(response) => Some(response.into_inner()),
        Err(_) => None,
    };

    // Fetch volumes
    let volumes = match zfs_client.list_volumes(ListVolumesRequest {}).await {
        Ok(response) => response.into_inner().volumes,
        Err(e) => return Err(e.message().to_string()),
    };

    // Fetch templates
    let templates = match zfs_client.list_templates(ListTemplatesRequest {}).await {
        Ok(response) => response.into_inner().templates,
        Err(e) => return Err(e.message().to_string()),
    };

    // Fetch import jobs (include completed for progress display)
    let import_jobs = match zfs_client
        .list_import_jobs(ListImportJobsRequest {
            include_completed: true,
        })
        .await
    {
        Ok(response) => response.into_inner().jobs,
        Err(_) => vec![],
    };

    Ok(StorageState {
        pool,
        volumes,
        templates,
        import_jobs,
    })
}

pub async fn action_worker(
    mut vm_client: Option<VmServiceClient<Channel>>,
    mut zfs_client: Option<ZfsServiceClient<Channel>>,
    mut action_rx: mpsc::UnboundedReceiver<Action>,
    result_tx: mpsc::UnboundedSender<ActionResult>,
) {
    while let Some(action) = action_rx.recv().await {
        let result = match action {
            // === VM Actions ===
            Action::Refresh => {
                if let Some(ref mut client) = vm_client {
                    match client.list_vms(ListVmsRequest {}).await {
                        Ok(response) => ActionResult::Refreshed(Ok(response.into_inner().vms)),
                        Err(e) => ActionResult::Refreshed(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::Refreshed(Err("VMM service not available".to_string()))
                }
            }
            Action::RefreshSystemInfo => {
                if let Some(ref mut client) = vm_client {
                    match client.get_system_info(GetSystemInfoRequest {}).await {
                        Ok(response) => {
                            ActionResult::SystemInfoRefreshed(Ok(response.into_inner()))
                        }
                        Err(e) => ActionResult::SystemInfoRefreshed(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::SystemInfoRefreshed(Err("VMM service not available".to_string()))
                }
            }
            Action::Start(id) => {
                if let Some(ref mut client) = vm_client {
                    match client.start_vm(StartVmRequest { id: id.clone() }).await {
                        Ok(_) => ActionResult::Started(id, Ok(())),
                        Err(e) => ActionResult::Started(id, Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::Started(id, Err("VMM service not available".to_string()))
                }
            }
            Action::Stop(id) => {
                if let Some(ref mut client) = vm_client {
                    match client
                        .stop_vm(StopVmRequest {
                            id: id.clone(),
                            timeout_seconds: 30,
                        })
                        .await
                    {
                        Ok(_) => ActionResult::Stopped(id, Ok(())),
                        Err(e) => ActionResult::Stopped(id, Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::Stopped(id, Err("VMM service not available".to_string()))
                }
            }
            Action::Kill(id) => {
                if let Some(ref mut client) = vm_client {
                    match client.kill_vm(KillVmRequest { id: id.clone() }).await {
                        Ok(_) => ActionResult::Killed(id, Ok(())),
                        Err(e) => ActionResult::Killed(id, Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::Killed(id, Err("VMM service not available".to_string()))
                }
            }
            Action::Delete(id) => {
                if let Some(ref mut client) = vm_client {
                    match client.delete_vm(DeleteVmRequest { id: id.clone() }).await {
                        Ok(_) => ActionResult::Deleted(id, Ok(())),
                        Err(e) => ActionResult::Deleted(id, Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::Deleted(id, Err("VMM service not available".to_string()))
                }
            }
            Action::Create(params) => {
                if let Some(ref mut client) = vm_client {
                    let user_data_content = match generate_user_data(&params, &result_tx).await {
                        Some(content) => Some(content),
                        None if params.user_data_mode != UserDataMode::None => continue,
                        None => None,
                    };

                    // Resolve disk path from ZFS volume/template
                    let disk_path = if let Some(ref mut zfs) = zfs_client {
                        // Get pool name
                        let pool_name = match zfs.get_pool_stats(GetPoolStatsRequest {}).await {
                            Ok(resp) => resp.into_inner().name,
                            Err(e) => {
                                let _ = result_tx.send(ActionResult::Created(Err(format!(
                                    "Failed to get pool info: {}",
                                    e.message()
                                ))));
                                continue;
                            }
                        };

                        // For templates, clone to a new volume first
                        let volume_name = match params.disk_source_type {
                            DiskSourceType::Template => {
                                // Generate volume name from VM name or template name
                                let new_vol_name = if let Some(ref vm_name) = params.name {
                                    vm_name.clone()
                                } else {
                                    // Generate a unique name based on template
                                    format!(
                                        "{}-{}",
                                        params.disk_name,
                                        &uuid::Uuid::new_v4().to_string()[..8]
                                    )
                                };

                                // Clone the template
                                if let Err(e) = zfs
                                    .clone_from_template(CloneFromTemplateRequest {
                                        template_name: params.disk_name.clone(),
                                        new_volume_name: new_vol_name.clone(),
                                    })
                                    .await
                                {
                                    let _ = result_tx.send(ActionResult::Created(Err(format!(
                                        "Failed to clone template: {}",
                                        e.message()
                                    ))));
                                    continue;
                                }
                                new_vol_name
                            }
                            DiskSourceType::Volume => params.disk_name.clone(),
                        };

                        format!("/dev/zvol/{}/{}", pool_name, volume_name)
                    } else {
                        // No ZFS client - can't resolve disk
                        let _ = result_tx.send(ActionResult::Created(Err(
                            "ZFS service not available for disk resolution".to_string(),
                        )));
                        continue;
                    };

                    let disks = vec![DiskConfig {
                        path: disk_path,
                        readonly: false,
                    }];

                    let config = VmConfig {
                        vcpus: params.vcpus,
                        memory_mb: params.memory_mb,
                        boot_mode: 1, // Always boot from disk
                        kernel: None,
                        initramfs: None,
                        cmdline: None,
                        disks,
                        nics: vec![NicConfig {
                            tap: None,
                            mac: None,
                        }],
                        user_data: user_data_content,
                        nested_virt: params.nested_virt,
                    };
                    match client
                        .create_vm(CreateVmRequest {
                            name: params.name,
                            config: Some(config),
                        })
                        .await
                    {
                        Ok(response) => ActionResult::Created(Ok(response.into_inner().id)),
                        Err(e) => ActionResult::Created(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::Created(Err("VMM service not available".to_string()))
                }
            }
            Action::OpenConsole { vm_id, vm_name } => {
                if let Some(ref mut client) = vm_client {
                    let (input_tx, input_rx) = mpsc::unbounded_channel::<Vec<u8>>();

                    let vm_id_clone = vm_id.clone();
                    let input_stream =
                        UnboundedReceiverStream::new(input_rx).map(move |data| ConsoleInput {
                            vm_id: String::new(),
                            data,
                        });

                    let initial_msg = ConsoleInput {
                        vm_id: vm_id_clone,
                        data: vec![],
                    };
                    let full_stream = tokio_stream::once(initial_msg).chain(input_stream);

                    match client.console(full_stream).await {
                        Ok(response) => {
                            let mut output_stream = response.into_inner();
                            let result_tx_clone = result_tx.clone();
                            let vm_id_for_close = vm_id.clone();

                            let _ = result_tx.send(ActionResult::ConsoleOpened {
                                vm_id,
                                vm_name,
                                input_tx,
                            });

                            tokio::spawn(async move {
                                while let Some(result) = output_stream.next().await {
                                    match result {
                                        Ok(output) => {
                                            if result_tx_clone
                                                .send(ActionResult::ConsoleOutput(output.data))
                                                .is_err()
                                            {
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            let _ =
                                                result_tx_clone.send(ActionResult::ConsoleClosed(
                                                    Some(e.message().to_string()),
                                                ));
                                            return;
                                        }
                                    }
                                }
                                let _ = result_tx_clone.send(ActionResult::ConsoleClosed(None));
                                drop(vm_id_for_close);
                            });

                            continue;
                        }
                        Err(e) => ActionResult::ConsoleClosed(Some(e.message().to_string())),
                    }
                } else {
                    ActionResult::ConsoleClosed(Some("VMM service not available".to_string()))
                }
            }

            // === Storage Actions ===
            Action::RefreshStorage => {
                if let Some(ref mut client) = zfs_client {
                    match refresh_storage(client).await {
                        Ok(state) => ActionResult::StorageRefreshed(Ok(state)),
                        Err(e) => ActionResult::StorageRefreshed(Err(e)),
                    }
                } else {
                    ActionResult::StorageRefreshed(Err("ZFS service not available".to_string()))
                }
            }
            Action::CreateVolume { name, size_bytes } => {
                if let Some(ref mut client) = zfs_client {
                    match client
                        .create_volume(CreateVolumeRequest {
                            name,
                            size_bytes,
                            volblocksize: None,
                        })
                        .await
                    {
                        Ok(_) => ActionResult::VolumeCreated(Ok(())),
                        Err(e) => ActionResult::VolumeCreated(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::VolumeCreated(Err("ZFS service not available".to_string()))
                }
            }
            Action::DeleteVolume(name) => {
                if let Some(ref mut client) = zfs_client {
                    match client.delete_volume(DeleteVolumeRequest { name }).await {
                        Ok(_) => ActionResult::VolumeDeleted(Ok(())),
                        Err(e) => ActionResult::VolumeDeleted(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::VolumeDeleted(Err("ZFS service not available".to_string()))
                }
            }
            Action::ResizeVolume { name, new_size } => {
                if let Some(ref mut client) = zfs_client {
                    match client
                        .resize_volume(ResizeVolumeRequest {
                            name,
                            new_size_bytes: new_size,
                        })
                        .await
                    {
                        Ok(_) => ActionResult::VolumeResized(Ok(())),
                        Err(e) => ActionResult::VolumeResized(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::VolumeResized(Err("ZFS service not available".to_string()))
                }
            }
            Action::ImportVolume {
                name,
                source,
                size_bytes,
            } => {
                if let Some(ref mut client) = zfs_client {
                    match client
                        .import_volume(ImportVolumeRequest {
                            name,
                            source,
                            size_bytes,
                        })
                        .await
                    {
                        Ok(response) => ActionResult::ImportStarted(Ok(response.into_inner().id)),
                        Err(e) => ActionResult::ImportStarted(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::ImportStarted(Err("ZFS service not available".to_string()))
                }
            }
            Action::CancelImport(job_id) => {
                if let Some(ref mut client) = zfs_client {
                    match client
                        .cancel_import_job(CancelImportJobRequest { id: job_id })
                        .await
                    {
                        Ok(_) => ActionResult::ImportCancelled(Ok(())),
                        Err(e) => ActionResult::ImportCancelled(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::ImportCancelled(Err("ZFS service not available".to_string()))
                }
            }
            Action::CreateSnapshot { volume, name } => {
                if let Some(ref mut client) = zfs_client {
                    match client
                        .create_snapshot(CreateSnapshotRequest {
                            volume_name: volume,
                            snapshot_name: name,
                        })
                        .await
                    {
                        Ok(_) => ActionResult::SnapshotCreated(Ok(())),
                        Err(e) => ActionResult::SnapshotCreated(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::SnapshotCreated(Err("ZFS service not available".to_string()))
                }
            }
            Action::DeleteSnapshot { volume, name } => {
                if let Some(ref mut client) = zfs_client {
                    match client
                        .delete_snapshot(DeleteSnapshotRequest {
                            volume_name: volume,
                            snapshot_name: name,
                        })
                        .await
                    {
                        Ok(_) => ActionResult::SnapshotDeleted(Ok(())),
                        Err(e) => ActionResult::SnapshotDeleted(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::SnapshotDeleted(Err("ZFS service not available".to_string()))
                }
            }
            Action::RollbackSnapshot { volume, name } => {
                if let Some(ref mut client) = zfs_client {
                    match client
                        .rollback_snapshot(RollbackSnapshotRequest {
                            volume_name: volume,
                            snapshot_name: name,
                        })
                        .await
                    {
                        Ok(_) => ActionResult::SnapshotRolledBack(Ok(())),
                        Err(e) => ActionResult::SnapshotRolledBack(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::SnapshotRolledBack(Err("ZFS service not available".to_string()))
                }
            }
            Action::CreateTemplate { volume, name } => {
                if let Some(ref mut client) = zfs_client {
                    match client
                        .create_template(CreateTemplateRequest {
                            volume_name: volume,
                            template_name: name,
                        })
                        .await
                    {
                        Ok(_) => ActionResult::TemplateCreated(Ok(())),
                        Err(e) => ActionResult::TemplateCreated(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::TemplateCreated(Err("ZFS service not available".to_string()))
                }
            }
            Action::DeleteTemplate(name) => {
                if let Some(ref mut client) = zfs_client {
                    match client.delete_template(DeleteTemplateRequest { name }).await {
                        Ok(_) => ActionResult::TemplateDeleted(Ok(())),
                        Err(e) => ActionResult::TemplateDeleted(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::TemplateDeleted(Err("ZFS service not available".to_string()))
                }
            }
            Action::CloneTemplate {
                template,
                new_volume,
            } => {
                if let Some(ref mut client) = zfs_client {
                    match client
                        .clone_from_template(CloneFromTemplateRequest {
                            template_name: template,
                            new_volume_name: new_volume,
                        })
                        .await
                    {
                        Ok(_) => ActionResult::VolumeCloned(Ok(())),
                        Err(e) => ActionResult::VolumeCloned(Err(e.message().to_string())),
                    }
                } else {
                    ActionResult::VolumeCloned(Err("ZFS service not available".to_string()))
                }
            }
        };
        if result_tx.send(result).is_err() {
            break;
        }
    }
}
