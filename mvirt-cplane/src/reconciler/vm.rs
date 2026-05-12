//! VM reconciler — converges desired VmData onto the owning node's
//! mvirt-vmm daemon via the reverse tunnel.
//!
//! State machine:
//!   Pending → Creating → Running ↔ Stopping → Stopped
//!   any → Failed (terminal until operator intervention)
//!
//! Dependencies:
//!   - VmStatus.node_id must be set (scheduler runs first; we no-op until
//!     it's assigned).
//!   - The boot Volume must be Ready with a path — we wait otherwise.
//!   - NIC is currently optional from the reconciler's POV: until the NIC
//!     reconciler is real, we either thread a TAP name through (if
//!     attached and the NIC reports one) or skip networking. The VM will
//!     boot disk-only in the latter case — useful for smoke testing.

use anyhow::Result;
use chrono::Utc;
use mvirt_daemon_protos::vmm::{
    BootMode, CreateVmRequest, DiskConfig, GetVmRequest, NicConfig, StartVmRequest, StopVmRequest,
    Vm, VmConfig, VmState,
};
use tonic::Code;
use tracing::{info, warn};

use super::Ctx;
use crate::command::{Command, VmDesiredState, VmPhase, VmStatus, VolumePhase};
use crate::state::ApiState;
use crate::tunnel::NodeHandle;

pub fn list_ids(state: &ApiState) -> Vec<String> {
    state.vm_ids()
}

/// Drive every VM that boots off the given volume. Called when the volume
/// reconciler writes a status update — that's the moment a VM blocked on
/// "volume not Ready yet" can finally proceed, and we don't want the
/// transition to wait for the 30s resync.
pub async fn reconcile_for_volume(ctx: &Ctx, volume_id: &str) -> Result<()> {
    let state = ctx.store.snapshot().await;
    let dependents: Vec<String> = state
        .vm_ids()
        .into_iter()
        .filter_map(|id| state.get_vm(&id))
        .filter(|vm| vm.spec.volume_id == volume_id)
        .map(|vm| vm.id)
        .collect();
    for id in dependents {
        let _ = reconcile(ctx, &id).await;
    }
    Ok(())
}

pub async fn reconcile(ctx: &Ctx, id: &str) -> Result<()> {
    let state = ctx.store.snapshot().await;
    let Some(vm) = state.get_vm(id) else {
        return Ok(()); // deleted; cleanup pending finalizer ADR
    };

    // Scheduler hasn't picked a node yet — nothing to do.
    let Some(node_id) = vm.status.node_id.as_deref() else {
        return Ok(());
    };

    let target_running = matches!(vm.spec.desired_state, VmDesiredState::Running);
    if at_target(&vm.status.phase, target_running) {
        return Ok(());
    }

    // Boot disk must exist and be Ready before we can wire it into the VM
    // config. The Volume reconciler runs ahead of us via the event loop;
    // if not yet Ready, we exit and the 30s resync (or the next
    // VolumeUpdated event) re-fires us.
    let Some(vol) = state.get_volume(&vm.spec.volume_id) else {
        warn!(vm = %id, volume = %vm.spec.volume_id, "boot volume missing; waiting");
        return Ok(());
    };
    if vol.status.phase != VolumePhase::Ready {
        return Ok(());
    }
    let disk_path = vol.status.path.clone();
    if disk_path.is_empty() {
        warn!(vm = %id, "volume Ready but no path; waiting for status writeback");
        return Ok(());
    }

    // NIC is optional in the reconciler: a missing or Pending NIC just
    // means the VM boots without networking for now. This keeps the path
    // unblocked while the NIC reconciler is still a stub.
    let nic_socket = state
        .get_nic(&vm.spec.nic_id)
        .filter(|n| !n.status.socket_path.is_empty())
        .map(|n| n.status.socket_path);

    let Some(node) = ctx.registry.get(node_id).await else {
        warn!(vm = %id, node = %node_id, "owning node not connected; will retry on resync");
        return Ok(());
    };

    info!(vm = %id, node = %node_id, name = %vm.spec.name, phase = ?vm.status.phase, "reconciling vm");

    let outcome = drive(
        &node,
        &vm,
        &disk_path,
        nic_socket.as_deref(),
        target_running,
    )
    .await;

    let cmd = match outcome {
        Ok(new_phase) => Command::UpdateVmStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            status: VmStatus {
                phase: new_phase,
                node_id: Some(node_id.to_string()),
                ip_address: vm.status.ip_address.clone(),
                message: None,
            },
        },
        Err(e) => Command::UpdateVmStatus {
            request_id: uuid::Uuid::new_v4().to_string(),
            id: id.to_string(),
            timestamp: Utc::now().to_rfc3339(),
            status: VmStatus {
                phase: VmPhase::Failed,
                node_id: Some(node_id.to_string()),
                ip_address: vm.status.ip_address.clone(),
                message: Some(e),
            },
        },
    };

    ctx.store
        .submit(cmd)
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("write vm status: {e}"))
}

/// True if `current` already matches what `target_running` asks for and
/// further work is wasted (or, for `Failed`, blocked until the operator
/// resets).
fn at_target(current: &VmPhase, target_running: bool) -> bool {
    matches!(
        (current, target_running),
        (VmPhase::Running, true) | (VmPhase::Stopped, false) | (VmPhase::Failed, _)
    )
}

/// Drive the VM one step closer to the target state. Returns the phase to
/// write back; the caller persists it via UpdateVmStatus.
async fn drive(
    node: &NodeHandle,
    vm: &crate::command::VmData,
    disk_path: &str,
    nic_socket: Option<&str>,
    target_running: bool,
) -> std::result::Result<VmPhase, String> {
    // get_or_create: idempotent against partial failures and node restarts.
    let current = match get_vm(node, &vm.id).await? {
        Some(v) => v,
        None => create_vm(node, vm, disk_path, nic_socket).await?,
    };

    let observed = VmState::try_from(current.state).unwrap_or(VmState::Unspecified);

    if target_running {
        match observed {
            VmState::Running => Ok(VmPhase::Running),
            VmState::Starting => Ok(VmPhase::Creating),
            VmState::Stopped | VmState::Unspecified => {
                start_vm(node, &vm.id).await?;
                Ok(VmPhase::Creating)
            }
            VmState::Stopping => Ok(VmPhase::Stopping),
        }
    } else {
        match observed {
            VmState::Stopped | VmState::Unspecified => Ok(VmPhase::Stopped),
            VmState::Stopping => Ok(VmPhase::Stopping),
            VmState::Running | VmState::Starting => {
                stop_vm(node, &vm.id).await?;
                Ok(VmPhase::Stopping)
            }
        }
    }
}

async fn get_vm(node: &NodeHandle, id: &str) -> std::result::Result<Option<Vm>, String> {
    let mut vmm = node.vmm.clone();
    match vmm.get_vm(GetVmRequest { id: id.to_string() }).await {
        Ok(resp) => Ok(Some(resp.into_inner())),
        Err(s) if s.code() == Code::NotFound => Ok(None),
        Err(s) => Err(format!("get_vm: {}", s.message())),
    }
}

async fn create_vm(
    node: &NodeHandle,
    vm: &crate::command::VmData,
    disk_path: &str,
    nic_socket: Option<&str>,
) -> std::result::Result<Vm, String> {
    let mut vmm = node.vmm.clone();

    let nics = nic_socket
        .map(|s| {
            vec![NicConfig {
                tap: None,
                mac: None,
                vhost_socket: Some(s.to_string()),
            }]
        })
        .unwrap_or_default();

    let config = VmConfig {
        vcpus: vm.spec.cpu_cores,
        memory_mb: vm.spec.memory_mb,
        // `image` is operator-provided and assumed to be a kernel path on
        // the node — the current convention in mvirt is direct-kernel
        // boot per CLAUDE.md. UEFI-from-disk variants would need a
        // separate spec field; not modelled yet.
        boot_mode: BootMode::Kernel as i32,
        kernel: if vm.spec.image.is_empty() {
            None
        } else {
            Some(vm.spec.image.clone())
        },
        initramfs: None,
        cmdline: None,
        disks: vec![DiskConfig {
            path: disk_path.to_string(),
            readonly: false,
        }],
        nics,
        user_data: None,
        nested_virt: false,
    };

    vmm.create_vm(CreateVmRequest {
        name: Some(vm.spec.name.clone()),
        config: Some(config),
    })
    .await
    .map(|r| r.into_inner())
    .map_err(|s| format!("create_vm: {}", s.message()))
}

async fn start_vm(node: &NodeHandle, id: &str) -> std::result::Result<(), String> {
    let mut vmm = node.vmm.clone();
    vmm.start_vm(StartVmRequest { id: id.to_string() })
        .await
        .map(|_| ())
        .map_err(|s| format!("start_vm: {}", s.message()))
}

async fn stop_vm(node: &NodeHandle, id: &str) -> std::result::Result<(), String> {
    let mut vmm = node.vmm.clone();
    vmm.stop_vm(StopVmRequest {
        id: id.to_string(),
        timeout_seconds: 30,
    })
    .await
    .map(|_| ())
    .map_err(|s| format!("stop_vm: {}", s.message()))
}
