# mvirt-net Architecture

This document describes the internal architecture of mvirt-net for developers.

## Overview

mvirt-net is a vhost-user backend for virtio-net devices. It provides:

1. **Control Plane**: gRPC API for managing networks and vNICs (async, tokio-based)
2. **Data Plane**: Per-vNIC Reactor threads for packet processing (sync, io_uring-based)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              mvirt-net                                      │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Control Plane (tokio async runtime)                                        │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌────────────┐            │
│  │  gRPC API  │  │   Store    │  │   Audit    │  │  Network   │            │
│  │  (tonic)   │  │  (SQLite)  │  │   Logger   │  │  Manager   │            │
│  └─────┬──────┘  └────────────┘  └────────────┘  └─────┬──────┘            │
│        │                                               │                    │
│        │                                          spawn│                    │
│        ▼                                               ▼                    │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  Data Plane (dedicated threads per NIC, io_uring event loops)               │
│                                                                             │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐             │
│  │  NIC Reactor    │  │  NIC Reactor    │  │  TUN Reactor    │             │
│  │                 │  │                 │  │  (global)       │             │
│  │ ┌─────────────┐ │  │ ┌─────────────┐ │  │ ┌─────────────┐ │             │
│  │ │ vhost-user  │ │  │ │ vhost-user  │ │  │ │             │ │             │
│  │ │   backend   │ │  │ │   backend   │ │  │ │  Hugepage   │ │             │
│  │ └──────┬──────┘ │  │ └──────┬──────┘ │  │ │   Buffers   │ │             │
│  │        │        │  │        │        │  │ │             │ │             │
│  │ ┌──────▼──────┐ │  │ ┌──────▼──────┐ │  │ └──────┬──────┘ │             │
│  │ │  io_uring   │ │  │ │  io_uring   │ │  │ ┌──────▼──────┐ │             │
│  │ │ event loop  │ │  │ │ event loop  │ │  │ │  io_uring   │ │             │
│  │ └─────────────┘ │  │ └─────────────┘ │  │ │ event loop  │ │             │
│  └────────┬────────┘  └────────┬────────┘  │ └─────────────┘ │             │
│           │                    │           └────────┬────────┘             │
│           │                    │                    │                      │
│           └───────── ReactorRegistry ───────────────┘                      │
│                    (mpsc + eventfd signaling)                              │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
          │                 │                         │
    vhost-user        vhost-user                     TUN
          │                 │                         │
    ┌─────▼─────┐     ┌─────▼─────┐          ┌───────▼───────┐
    │   VM #1   │     │   VM #2   │          │ Linux Kernel  │
    └───────────┘     └───────────┘          │ (NAT/Internet)│
                                             └───────────────┘
```

## Per-NIC Reactor Architecture

Each vNIC gets its own dedicated Reactor thread:

- **Isolation**: A bug in one Reactor doesn't affect others
- **Parallelism**: Multiple NICs process packets concurrently
- **No contention**: Each Reactor has its own io_uring instance and routing table

```rust
// Each NIC creates its own Router (which spawns a Reactor thread)
pub async fn create_nic_router(&self, nic: &NicData, network: &NetworkData) -> Result<()> {
    let router = Router::with_shared_registry(
        &tun_name,
        Some((tun_ip, prefix_len)),
        TUN_BUFFER_SIZE,
        TUN_BUFFER_COUNT,
        TUN_BUFFER_COUNT,
        Some(vhost_config),
        Arc::clone(&self.registry),  // Shared registry for inter-reactor comms
    ).await?;
    // ...
}
```

## Components

### Control Plane

#### gRPC Service (`grpc/service.rs`)

Handles network and vNIC management:

```rust
impl NetService for NetServiceImpl {
    async fn create_nic(&self, request: Request<CreateNicRequest>)
        -> Result<Response<Nic>, Status>;
    // ...
}
```

Flow for CreateNic:
1. Validate request
2. Allocate IP address from network subnet
3. Generate MAC address (if not provided)
4. Store NIC in database
5. Create Router (spawns Reactor thread + vhost-user daemon)
6. Return NIC with socket path

#### Storage (`grpc/storage.rs`)

SQLite persistence with rusqlite:

```rust
pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    pub fn create_network(&self, entry: &NetworkData) -> Result<()>;
    pub fn create_nic(&self, entry: &NicData) -> Result<()>;
    // ...
}
```

Tables:
- `networks`: Network definitions
- `nics`: vNIC definitions with IP assignments

#### NetworkManager (`grpc/manager.rs`)

Manages Router lifecycle:

```rust
pub struct NetworkManager {
    registry: Arc<ReactorRegistry>,    // Shared across all Reactors
    storage: Arc<Storage>,
    tun_router: Mutex<Option<Router>>, // Global TUN for public networks
    nics: Mutex<HashMap<Uuid, ManagedNic>>,
}

struct ManagedNic {
    data: NicData,
    router: Router,      // Each NIC has its own Router
    table_id: Uuid,
}
```

### Data Plane

#### Router (`router.rs`)

Combines TUN device with optional vhost-user backend:

```rust
pub struct Router {
    reactor_handle: ReactorHandle,
    reactor_thread: JoinHandle<()>,
    vhost_thread: Option<JoinHandle<io::Result<()>>>,
    tun_name: String,
    registry: Arc<ReactorRegistry>,
    reactor_id: ReactorId,
}
```

Each Router:
1. Creates a TUN device
2. Allocates hugepage-backed buffers
3. Spawns a Reactor thread (io_uring event loop)
4. Optionally spawns a vhost-user daemon thread

#### Reactor (`reactor/mod.rs`)

The core packet processing engine using io_uring:

```rust
pub struct Reactor<RX, TX> {
    rx_queue: RX,
    tx_queue: TX,
    event_fd: RawFd,
    command_rx: Receiver<ReactorCommand>,
    routing_tables: RoutingTables,
    reactor_id: ReactorId,
    registry: Option<Arc<ReactorRegistry>>,
    packet_rx: Option<Receiver<PacketRef>>,
    completion_rx: Option<Receiver<CompletionNotify>>,
    nic_config: Option<NicConfig>,
}
```

Main loop (`Reactor::run()`):
```rust
loop {
    // Wait for io_uring completions
    ring.submit_and_wait(1)?;

    for cqe in ring.completion() {
        match user_data_flags(cqe) {
            EVENT_FLAG => handle_commands_and_handshake(),
            RX_FLAG => handle_tun_rx(cqe),      // Packet from TUN
            VHOST_TX_FLAG => handle_vhost_tx(cqe), // Packet from VM
            // ...
        }
    }
}
```

#### Protocol Handlers (`reactor/*.rs`)

- **`arp.rs`**: Responds to ARP requests for gateway (169.254.0.1)
- **`dhcp.rs`**: DHCPv4 server (assigns /32 addresses)
- **`dhcpv6.rs`**: DHCPv6 server (assigns /128 addresses)
- **`icmpv6.rs`**: NDP responder (Neighbor Solicitation for fe80::1) and Router Advertisements

#### Inter-Reactor Communication (`inter_reactor.rs`)

Zero-copy packet forwarding between Reactors:

```rust
/// Packet reference for zero-copy forwarding
pub struct PacketRef {
    pub id: PacketId,
    iovecs: [libc::iovec; MAX_PACKET_IOVECS],  // Points to guest memory
    iovecs_len: usize,
    pub source: PacketSource,
    pub keep_alive: Option<Arc<dyn Any + Send + Sync>>,  // Prevents unmap
}

/// Completion notification sent back to source
pub enum CompletionNotify {
    VhostTxComplete { packet_id, head_index, total_len, result },
    TunRxComplete { packet_id, chain_id, result },
    VhostToVhostComplete { packet_id, head_index, total_len, result },
}
```

#### ReactorRegistry (`reactor/registry.rs`)

Central registry for inter-reactor packet routing:

```rust
pub struct ReactorRegistry {
    reactors: RwLock<HashMap<ReactorId, ReactorInfo>>,
}

pub struct ReactorInfo {
    pub id: ReactorId,
    pub notify_fd: RawFd,                    // eventfd for wakeup
    pub packet_tx: Sender<PacketRef>,        // Channel for incoming packets
    pub completion_tx: Sender<CompletionNotify>,
    pub interface_type: InterfaceType,
    pub mac: Option<[u8; 6]>,                // For Ethernet header construction
}
```

#### Routing (`routing.rs`)

LPM (Longest Prefix Match) routing tables:

```rust
pub struct RoutingTables {
    tables: HashMap<Uuid, LpmTable>,
    default_table: Option<Uuid>,
}

pub struct LpmTable {
    id: Uuid,
    name: String,
    v4_trie: IpLookupTable<Ipv4Addr, RouteTarget>,
    v6_trie: IpLookupTable<Ipv6Addr, RouteTarget>,
}

pub enum RouteTarget {
    Reactor { id: ReactorId },   // Forward to another Reactor
    Tun { if_index: u32 },       // Send to TUN (kernel routing)
    Drop,
}
```

## Packet Flow Examples

### VM → ARP for Gateway

```
VM: ARP Who-has 169.254.0.1?
    │
    ▼
Reactor: io_uring CQE for vhost TX
    │
    ▼
Reactor: arp::handle_arp_request() recognizes gateway
    │
    ▼
Reactor: builds ARP reply (169.254.0.1 is-at 02:00:00:00:00:01)
    │
    ▼
Reactor: writes to vhost RX vring
    │
    ▼
VM: Receives ARP reply
```

### VM → DHCPv4

```
VM: DHCPDISCOVER (broadcast)
    │
    ▼
Reactor: io_uring CQE for vhost TX
    │
    ▼
Reactor: dhcp::handle_dhcp() builds DHCPOFFER
    │  - yiaddr: 10.0.0.5
    │  - router: 169.254.0.1
    │  - subnet: 255.255.255.255 (/32)
    │
    ▼
Reactor: writes to vhost RX vring
    │
    ▼
VM: DHCPREQUEST → Reactor: DHCPACK
    │
    ▼
VM: Configures interface
```

### VM1 → VM2 (Inter-VM via ReactorRegistry)

```
VM1: IP packet to 10.0.0.6
    │
    ▼
Reactor1: io_uring CQE for vhost TX
    │
    ▼
Reactor1: routing_tables.lookup(10.0.0.6)
    │  - Returns RouteTarget::Reactor { id: reactor2_id }
    │
    ▼
Reactor1: registry.get(reactor2_id)
    │  - Gets packet_tx channel
    │
    ▼
Reactor1: packet_tx.send(PacketRef { iovecs pointing to guest memory })
    │
    ▼
Reactor1: write(reactor2.notify_fd, 1)  // Wake up Reactor2
    │
    ▼
Reactor2: io_uring CQE for eventfd
    │
    ▼
Reactor2: packet_rx.recv() → PacketRef
    │
    ▼
Reactor2: Copy packet to VM2's RX vring (with new Ethernet header)
    │
    ▼
Reactor2: completion_tx.send(VhostToVhostComplete)
    │
    ▼
Reactor1: Returns descriptor to VM1's used ring
    │
    ▼
VM2: Receives packet
```

### VM → Internet (via TUN)

```
VM: IP packet to 8.8.8.8
    │
    ▼
NIC Reactor: routing_tables.lookup(8.8.8.8)
    │  - Returns RouteTarget::Reactor { id: tun_reactor_id }
    │
    ▼
NIC Reactor: Forward PacketRef to TUN Reactor
    │
    ▼
TUN Reactor: io_uring writev() to TUN fd
    │  - Strips Ethernet header, patches virtio_net_hdr
    │
    ▼
Linux Kernel: Routes packet (NAT via iptables)
    │
    ▼
Internet
```

## Memory Layout

### Hugepage Buffer Pool

```rust
pub struct HugePagePool {
    ptr: *mut u8,
    size: usize,
}

// Allocated at Router creation
let buffers = HugePagePool::new((rx_count + tx_count) * buf_size)?;
```

Benefits:
- Reduced TLB misses (2MB pages vs 4KB)
- Registered with io_uring for zero-copy I/O
- Page-aligned for efficient DMA

### vhost-user Shared Memory

```
┌─────────────────────────────────────────────────────────────────┐
│                    Guest Memory Region                          │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  Virtio-net Device                                       │  │
│  │                                                          │  │
│  │  ┌─────────────┐  ┌─────────────┐                       │  │
│  │  │  RX Vring   │  │  TX Vring   │                       │  │
│  │  │ Descriptors │  │ Descriptors │                       │  │
│  │  │ Available   │  │ Available   │                       │  │
│  │  │ Used        │  │ Used        │                       │  │
│  │  └─────────────┘  └─────────────┘                       │  │
│  │                                                          │  │
│  │  ┌───────────────────────────────────────────────────┐  │  │
│  │  │              Packet Buffers                       │  │  │
│  │  └───────────────────────────────────────────────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
           │
           │ mmap (shared with backend via vhost-user protocol)
           ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Reactor Thread                               │
│                                                                 │
│  PacketRef.iovecs point directly to guest memory                │
│  keep_alive holds Arc<GuestMemory> to prevent unmap             │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

## io_uring Operations

The Reactor uses io_uring for all async I/O:

| Operation | io_uring opcode | user_data flag |
|-----------|-----------------|----------------|
| TUN read (RX) | `ReadFixed` | `USER_DATA_RX_FLAG` |
| TUN write (TX) | `Writev` | (chain_id only) |
| vhost TX to TUN | `Writev` | `USER_DATA_VHOST_TX_FLAG` |
| Eventfd read | `Read` | `USER_DATA_EVENT_FLAG` |
| TUN poll (EAGAIN) | `PollAdd` | `USER_DATA_TUN_POLL_FLAG` |

Buffers are registered with `register_buffers()` for zero-copy `ReadFixed` operations.

## Dependencies

| Crate | Purpose |
|-------|---------|
| io-uring | Async I/O |
| vhost | vhost-user protocol |
| vhost-user-backend | Backend framework |
| vm-memory | Guest memory abstraction |
| virtio-queue | Virtio queue handling |
| smoltcp | Packet parsing/building |
| dhcproto | DHCPv4/v6 protocol |
| ip_network_table | LPM routing tables |
| tonic | gRPC server |
| rusqlite | SQLite storage |
| rtnetlink | Kernel route management |

## Key Differences from Traditional Designs

| Aspect | Traditional (e.g., OVS) | mvirt-net |
|--------|------------------------|-----------|
| Threading | Single data plane thread | Per-NIC Reactor threads |
| I/O Model | epoll/poll | io_uring |
| Packet Copy | Multiple copies | Zero-copy via iovecs |
| Memory | Regular pages | Hugepages |
| L2/L3 | L2 switching | Pure L3 routing |
