# uos (microOS) - Design Document

## Overview

uos is the init system for mvirt microVMs that run isolated pods (container groups sharing namespaces). It replaces the existing pideisn implementation.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         Host (mvirt-vmm)                         │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐       │
│  │  VmService   │    │  PodService  │    │  mvirt-net   │       │
│  │  (existing)  │    │    (new)     │    │  (existing)  │       │
│  └──────────────┘    └──────┬───────┘    └──────────────┘       │
│                             │ vsock                              │
└─────────────────────────────┼───────────────────────────────────┘
                              │
┌─────────────────────────────┼───────────────────────────────────┐
│                     MicroVM │ (cloud-hypervisor)                 │
│                             ▼                                    │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                      uos (PID 1)                          │   │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────────┐  │   │
│  │  │ Image   │  │  Task   │  │   Pod   │  │    API      │  │   │
│  │  │ Service │  │ Service │  │ Service │  │   Server    │  │   │
│  │  └─────────┘  └─────────┘  └─────────┘  └─────────────┘  │   │
│  └──────────────────────────────────────────────────────────┘   │
│                             │                                    │
│           ┌─────────────────┼─────────────────┐                 │
│           ▼                 ▼                 ▼                  │
│    ┌──────────┐      ┌──────────┐      ┌──────────┐            │
│    │Container1│      │Container2│      │Container3│            │
│    │ (youki)  │      │ (youki)  │      │ (youki)  │            │
│    └──────────┘      └──────────┘      └──────────┘            │
│         └──────────────────┴──────────────────┘                 │
│                    Shared Network Namespace                      │
└─────────────────────────────────────────────────────────────────┘
```

## Service Architecture (Actor Pattern)

Based on FeOS patterns, each service follows the Actor pattern:

```
API Handler  →  Dispatcher  →  Worker
    │              │             │
    │   mpsc       │   spawn     │
    └──────────────┴─────────────┘
         Command Queue      Async Tasks
```

### Services

1. **Image Service** - OCI image pulling and storage
   - Orchestrator: Coordinates pull operations
   - Puller: Downloads from OCI registry (oci_distribution crate)
   - FileStore: Extracts layers to rootfs

2. **Task Service** - Low-level OCI runtime interface
   - Wraps youki subprocess
   - Handles: create, start, kill, delete, wait
   - Uses waitpid for exit detection

3. **Pod Service** - High-level pod/container management
   - Coordinates Image + Task services
   - Generates OCI runtime specs
   - Manages shared namespaces

## Dual-Mode Operation

uos runs in two modes:

### PID 1 Mode (in MicroVM)
```rust
if std::process::id() == 1 {
    mount_virtual_filesystems();  // /proc, /sys, /dev, /run, /tmp, /sys/fs/cgroup
    configure_network();          // DHCP via rtnetlink
    start_vsock_server();         // vsock CID:any Port:1024
}
```

### Local Mode (for development)
```rust
else {
    prctl::set_child_subreaper(true)?;  // Handle orphaned children
    start_unix_socket_server();          // /tmp/uos.sock
}
```

## Data Flow

### Image Pull
```
1. PodService receives CreatePod(image: "alpine:latest")
2. PodService → ImageService: PullImage
3. ImageService.Orchestrator → ImageService.Puller: fetch layers
4. Puller: oci_distribution client → registry
5. Puller → FileStore: store layers
6. FileStore: extract gzip → /run/images/{uuid}/rootfs
7. ImageService → PodService: image ready
```

### Container Start
```
1. PodService: generate OCI runtime spec (config.json)
2. PodService → TaskService: Create(bundle_path)
3. TaskService: youki create --bundle <path> --pid-file <file> <id>
4. TaskService: read PID from file
5. PodService → TaskService: Start(id)
6. TaskService: youki start <id>
7. TaskService: spawn waitpid background task
8. On exit: TaskService → PodService: ContainerStopped event
```

## Directory Structure

### In MicroVM (/run)
```
/run/
├── images/{image_uuid}/
│   ├── config.json       # OCI image config
│   ├── metadata.json     # image reference
│   └── rootfs/           # extracted layers
└── pods/{pod_id}/{container_id}/
    ├── config.json       # OCI runtime spec
    └── rootfs/           # symlink or bind to image rootfs
```

### Local Development (/tmp/uos)
```
/tmp/uos/
├── images/
└── pods/
```

## API (Protobuf over vsock/Unix socket)

```protobuf
service UosService {
  // Pod lifecycle
  rpc CreatePod(CreatePodRequest) returns (Pod);
  rpc StartPod(StartPodRequest) returns (Pod);
  rpc StopPod(StopPodRequest) returns (Pod);
  rpc DeletePod(DeletePodRequest) returns (Empty);
  rpc GetPod(GetPodRequest) returns (Pod);
  rpc ListPods(Empty) returns (ListPodsResponse);

  // Container operations
  rpc Logs(LogsRequest) returns (stream LogsResponse);
  rpc Exec(stream ExecInput) returns (stream ExecOutput);

  // System
  rpc Shutdown(ShutdownRequest) returns (Empty);
  rpc Health(Empty) returns (HealthResponse);
}

message CreatePodRequest {
  string id = 1;
  string name = 2;
  repeated ContainerSpec containers = 3;
}

message ContainerSpec {
  string id = 1;
  string name = 2;
  string image = 3;           // e.g. "docker.io/library/alpine:latest"
  repeated string command = 4;
  repeated string args = 5;
  repeated string env = 6;
  string working_dir = 7;
}
```

## youki Integration

youki is invoked as a subprocess:

```rust
const YOUKI_BIN: &str = "/usr/bin/youki";

// Create container (paused state)
Command::new(YOUKI_BIN)
    .args(["create", "--bundle", bundle_path, "--pid-file", pid_file, container_id])
    .status().await?;

// Start container
Command::new(YOUKI_BIN)
    .args(["start", container_id])
    .status().await?;

// Wait for exit (background task)
let status = waitpid(Pid::from_raw(pid), None);
```

## OCI Runtime Spec Generation

```rust
fn generate_runtime_spec(container: &ContainerSpec, image_config: &ImageConfig) -> OciSpec {
    OciSpec {
        oci_version: "1.0.2",
        process: Process {
            args: container.command.clone(),
            env: container.env.clone(),
            cwd: container.working_dir.clone(),
            user: User { uid: 0, gid: 0 },
        },
        root: Root {
            path: "rootfs",
            readonly: false,
        },
        mounts: vec![
            Mount { destination: "/proc", type_: "proc", source: "proc" },
            Mount { destination: "/dev", type_: "tmpfs", source: "tmpfs" },
            Mount { destination: "/sys", type_: "sysfs", source: "sysfs" },
        ],
        linux: Linux {
            namespaces: vec![
                Namespace { type_: "pid" },
                Namespace { type_: "mount" },
                Namespace { type_: "ipc" },
                Namespace { type_: "uts" },
                // network namespace shared with pod
            ],
        },
    }
}
```

## Dependencies

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
tokio-vsock = "0.5"
nix = { version = "0.29", features = ["mount", "signal", "sched", "fs", "process"] }
prost = "0.13"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
oci-distribution = "0.11"
flate2 = "1"
tar = "0.4"
log = "0.4"
anyhow = "1"
thiserror = "2"
uuid = { version = "1", features = ["v4"] }

# Network (from pideisn, for PID 1 mode)
rtnetlink = "0.14"
dhcproto = "0.14"
socket2 = "0.5"
```

## Implementation Phases

### Phase 1: Local Development (no VM)
1. Project setup, main.rs with dual-mode
2. Task Service (youki integration)
3. Image Service (registry client + layer extraction)
4. Pod Service (container lifecycle)
5. API Server (Unix socket)

### Phase 2: VM Integration
6. Mount/Network (from pideisn)
7. vsock Server
8. mvirt-vmm PodService + vsock client
9. CLI extension

### Phase 3: Polish
10. Shared namespaces (pod networking)
11. Graceful shutdown
12. Logging integration

## Reference Implementation

FeOS (`~/feos`) provides reference patterns:
- `feos/src/main.rs` - PID 1 handling
- `feos/src/setup.rs` - Service initialization
- `feos/services/task-service/src/worker.rs` - youki integration
- `feos/services/image-service/src/worker.rs` - Image pulling
- `feos/services/image-service/src/filestore.rs` - Layer extraction
