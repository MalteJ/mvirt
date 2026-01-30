# End-to-End Test Plan

This describes a full end-to-end test flow covering the core mvirt resource lifecycle. All calls target the REST API at `http://<api-host>:8080/v1`.

## Prerequisites

- mvirt-api running with at least one node registered
- mvirt-node, mvirt-vmm, mvirt-zfs, mvirt-ebpf running on that node
- Network connectivity between API and node
- A reachable QCOW2 image URL for template import (e.g. Debian cloud image)
- `curl` and `jq` available

## 1. Health and Cluster Checks

| Step | Method | Endpoint | Expect |
|------|--------|----------|--------|
| API is reachable | `GET` | `/version` | 200 |
| Raft leader elected | `GET` | `/controlplane` | `leader_id` is non-null |
| At least one node registered | `GET` | `/nodes` | Array length >= 1, pick first node ID |

## 2. Project Setup

| Step | Method | Endpoint | Body | Expect |
|------|--------|----------|------|--------|
| Create project | `POST` | `/projects` | `{"id":"e2etest","name":"E2E Test Project"}` | 200, `id == "e2etest"` |

## 3. Networking

| Step | Method | Endpoint | Body | Expect |
|------|--------|----------|------|--------|
| Create network | `POST` | `/projects/e2etest/networks` | `{"name":"e2e-net","ipv4Enabled":true,"ipv4Prefix":"10.100.0.0/24"}` | 200, note `id` |
| Create security group | `POST` | `/projects/e2etest/security-groups` | `{"name":"e2e-sg"}` | 200, note `id` (auto-creates egress allow-all rules) |
| Add SSH ingress rule | `POST` | `/security-groups/{sg_id}/rules` | `{"direction":"INGRESS","protocol":"TCP","portStart":22,"portEnd":22,"cidr":"0.0.0.0/0"}` | 200 |
| Create NIC | `POST` | `/projects/e2etest/nics` | `{"networkId":"{net_id}","name":"e2e-nic","securityGroupId":"{sg_id}"}` | 200, note `id` |

## 4. Storage

| Step | Method | Endpoint | Body | Expect |
|------|--------|----------|------|--------|
| Import template | `POST` | `/projects/e2etest/templates/import` | `{"nodeId":"{node_id}","name":"debian-13","url":"https://...qcow2"}` | 200, note import job `id` |
| Poll import job | `GET` | `/import-jobs/{job_id}` | - | Poll until `state == "COMPLETED"` (timeout ~5min) |
| Verify template exists | `GET` | `/projects/e2etest/templates` | - | Template with matching name present, note `id` and `nodeId` |
| Create volume from template | `POST` | `/projects/e2etest/volumes` | `{"nodeId":"{template_node_id}","name":"e2e-vol","sizeBytes":10737418240,"templateId":"{template_id}"}` | 200, note `id` |

**Note:** The volume must be created on the same node as the template (clone is local to ZFS).

## 5. VM Lifecycle

| Step | Method | Endpoint | Body | Expect |
|------|--------|----------|------|--------|
| Create VM | `POST` | `/projects/e2etest/vms` | `{"name":"e2e-vm","config":{"vcpus":2,"memoryMb":1024,"volumeId":"{vol_id}","nicId":"{nic_id}","image":"debian-13"}}` | 200, note `id` |
| Start VM | `POST` | `/vms/{vm_id}/start` | - | 200 |
| Poll until running | `GET` | `/vms/{vm_id}` | - | `state == "RUNNING"` (timeout ~60s) |
| Stop VM | `POST` | `/vms/{vm_id}/stop` | - | 200 |
| Poll until stopped | `GET` | `/vms/{vm_id}` | - | `state == "STOPPED"` |
| Restart VM | `POST` | `/vms/{vm_id}/start` | - | 200 |
| Poll until running | `GET` | `/vms/{vm_id}` | - | `state == "RUNNING"` |
| Kill VM | `POST` | `/vms/{vm_id}/kill` | - | 200 |
| Poll until stopped | `GET` | `/vms/{vm_id}` | - | `state == "STOPPED"` |

## 6. Snapshots

| Step | Method | Endpoint | Body | Expect |
|------|--------|----------|------|--------|
| Create snapshot | `POST` | `/volumes/{vol_id}/snapshots` | `{"name":"pre-cleanup"}` | 200 |
| Verify snapshot | `GET` | `/volumes/{vol_id}` | - | `snapshots` array length >= 1 |

## 7. Audit Log

| Step | Method | Endpoint | Expect |
|------|--------|----------|--------|
| Query logs for VM | `GET` | `/logs?object_id={vm_id}` | `logs` array length >= 1 (entries for create, start, stop, etc.) |

## 8. Cleanup (reverse creation order)

| Step | Method | Endpoint | Expect |
|------|--------|----------|--------|
| Delete VM | `DELETE` | `/vms/{vm_id}` | 204 |
| Delete volume | `DELETE` | `/volumes/{vol_id}` | 204 |
| Delete NIC | `DELETE` | `/nics/{nic_id}` | 204 |
| Delete security group | `DELETE` | `/security-groups/{sg_id}` | 204 |
| Delete network | `DELETE` | `/networks/{net_id}` | 204 |
| Delete project | `DELETE` | `/projects/e2etest` | 204 |
| Verify project gone | `GET` | `/projects/e2etest` | 404 |

## Expected Result

All steps return their expected status codes. Resources are created, lifecycle operations work, and cleanup leaves no orphaned resources. A full run exercises: Raft consensus, node scheduling, ZFS storage, cloud-hypervisor lifecycle, eBPF networking, and audit logging.
