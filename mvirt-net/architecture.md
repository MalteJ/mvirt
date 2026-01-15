# mvirt-net Architecture

This document describes the internal architecture of mvirt-net for developers.

## Overview

mvirt-net is a vhost-user backend for virtio-net devices. It provides:

1. **Control Plane**: gRPC API for managing networks and vNICs (async, tokio-based)
2. **Data Plane**: Per-vNIC worker threads for packet processing (sync, shared-nothing)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              mvirt-net                                        │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│  Control Plane (tokio async runtime)                                         │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌────────────┐             │
│  │  gRPC API  │  │   Store    │  │   Audit    │  │  Worker    │             │
│  │  (tonic)   │  │  (SQLite)  │  │   Logger   │  │  Manager   │             │
│  └─────┬──────┘  └────────────┘  └────────────┘  └─────┬──────┘             │
│        │                                               │                      │
│        │                                          spawn│                      │
│        ▼                                               ▼                      │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                               │
│  Data Plane (dedicated threads, no tokio)                                    │
│                                                                               │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐              │
│  │  vNIC Worker    │  │  vNIC Worker    │  │  vNIC Worker    │              │
│  │                 │  │                 │  │                 │              │
│  │ ┌─────────────┐ │  │ ┌─────────────┐ │  │ ┌─────────────┐ │              │
│  │ │ vhost-user  │ │  │ │ vhost-user  │ │  │ │ vhost-user  │ │              │
│  │ │   backend   │ │  │ │   backend   │ │  │ │   backend   │ │              │
│  │ └──────┬──────┘ │  │ └──────┬──────┘ │  │ └──────┬──────┘ │              │
│  │        │        │  │        │        │  │        │        │              │
│  │ ┌──────▼──────┐ │  │ ┌──────▼──────┐ │  │ ┌──────▼──────┐ │              │
│  │ │   Packet    │ │  │ │   Packet    │ │  │ │   Packet    │ │              │
│  │ │  Processor  │ │  │ │  Processor  │ │  │ │  Processor  │ │              │
│  │ └─────────────┘ │  │ └─────────────┘ │  │ └─────────────┘ │              │
│  └────────┬────────┘  └────────┬────────┘  └────────┬────────┘              │
│           │                    │                    │                        │
│           └──────────crossbeam channels─────────────┘                        │
│                                │                                              │
│                   ┌────────────▼────────────┐                                │
│                   │   TUN IO Thread         │                                │
│                   │   (mvirt-net device)    │                                │
│                   └────────────┬────────────┘                                │
│                                │                                              │
└────────────────────────────────┼──────────────────────────────────────────────┘
                                 │
                                 ▼
                        Linux Kernel Routing
                           (NAT/Internet)
```

## Shared-Nothing Architecture

Each vNIC worker thread is completely independent:

- **No shared state**: Each worker has its own copy of configuration
- **No locks between workers**: Eliminates contention
- **Message passing for routing**: crossbeam channels for inter-vNIC packets

This design prioritizes:
1. **Simplicity**: No complex synchronization
2. **Isolation**: A bug in one worker doesn't affect others
3. **Predictable latency**: No lock contention

## Components

### Control Plane

#### gRPC Service (`grpc.rs`)

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
4. Create vhost-user socket
5. Store NIC in database
6. Spawn worker thread
7. Return NIC with socket path

#### Store (`store.rs`)

SQLite persistence with sqlx:

```rust
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn create_network(&self, entry: &NetworkEntry) -> Result<()>;
    pub async fn create_nic(&self, entry: &NicEntry) -> Result<()>;
    pub async fn allocate_address(&self, network_id: &str, address: &str, nic_id: &str) -> Result<()>;
    // ...
}
```

Tables:
- `networks`: Network definitions
- `nics`: vNIC definitions
- `address_allocations`: Tracks assigned IPs (prevents conflicts)
- `routed_prefixes`: Tracks routed prefixes for routing table

#### Worker Manager (`dataplane/mod.rs`)

Manages worker thread lifecycle:

```rust
pub struct WorkerManager {
    workers: HashMap<String, WorkerHandle>,
    router: Arc<Router>,
}

impl WorkerManager {
    pub fn spawn_worker(&mut self, nic: NicEntry, network: NetworkEntry) -> Result<()>;
    pub fn stop_worker(&mut self, nic_id: &str) -> Result<()>;
}
```

### Data Plane

#### Worker Thread (`dataplane/worker.rs`)

Main loop (blocking, no async):

```rust
impl NicWorker {
    pub fn run(mut self) {
        loop {
            // 1. Poll vhost-user for packets from VM
            self.poll_rx();

            // 2. Poll router channel for packets from other vNICs
            self.poll_router();

            // 3. Send periodic Router Advertisements (if IPv6)
            self.maybe_send_ra();

            // 4. Check for shutdown signal
            if self.should_shutdown() {
                break;
            }
        }
    }
}
```

#### vhost-user Backend (`dataplane/vhost.rs`)

Implements `VhostUserBackendMut` trait from rust-vmm:

```rust
impl VhostUserBackendMut<VringRwLock<GuestMemoryMmap>, ()> for NetBackend {
    fn process_queue(&mut self, mem: &GuestMemoryMmap, vring: &mut VringRwLock<GuestMemoryMmap>)
        -> Result<bool>;
}
```

Virtio-net has two queues:
- **RX queue** (index 0): Packets from backend to VM
- **TX queue** (index 1): Packets from VM to backend

#### Packet Processing (`dataplane/packet.rs`)

Uses smoltcp for packet parsing:

```rust
pub fn process_ethernet_frame(frame: &[u8]) -> PacketAction {
    let eth = EthernetFrame::new_checked(frame)?;

    match eth.ethertype() {
        EtherType::Arp => process_arp(eth.payload()),
        EtherType::Ipv4 => process_ipv4(eth.payload()),
        EtherType::Ipv6 => process_ipv6(eth.payload()),
        _ => PacketAction::Drop,
    }
}
```

#### ARP Responder (`dataplane/arp.rs`)

Responds to ARP requests for gateway (169.254.0.1):

```rust
pub fn handle_arp(request: &[u8], gateway_mac: &[u8; 6]) -> Option<Vec<u8>> {
    let arp = ArpPacket::new_checked(request)?;

    // Only respond if target is gateway
    if arp.target_protocol_addr() != GATEWAY_IPV4 {
        return None;
    }

    // Build ARP reply
    Some(build_arp_reply(arp, gateway_mac))
}
```

#### NDP Responder (`dataplane/ndp.rs`)

Handles Neighbor Discovery and Router Advertisements:

```rust
// Respond to Neighbor Solicitation for fe80::1
pub fn handle_ns(request: &[u8], gateway_mac: &[u8; 6]) -> Option<Vec<u8>>;

// Build periodic Router Advertisement
pub fn build_ra(config: &RaConfig) -> Vec<u8>;
```

RA Configuration:
- M flag = 1 (Managed address configuration via DHCPv6)
- O flag = 1 (Other configuration via DHCPv6)
- Router lifetime = 1800s
- Prefix with L=1 (on-link), A=0 (no SLAAC)

#### DHCPv4 Server (`dataplane/dhcpv4.rs`)

Uses dhcproto crate:

```rust
pub fn handle_dhcp(request: &[u8], config: &DhcpConfig) -> Option<Vec<u8>> {
    let msg = Message::decode(request)?;

    match msg.opts().msg_type()? {
        MessageType::Discover => build_offer(msg, config),
        MessageType::Request => build_ack(msg, config),
        _ => None,
    }
}
```

DHCP Options sent:
- Subnet mask: 255.255.255.255 (/32)
- Router: 169.254.0.1
- DNS servers: from network config
- Lease time: 0xFFFFFFFF (infinite)

#### DHCPv6 Server (`dataplane/dhcpv6.rs`)

Uses dhcproto crate:

```rust
pub fn handle_dhcpv6(request: &[u8], config: &Dhcpv6Config) -> Option<Vec<u8>> {
    let msg = v6::Message::decode(request)?;

    match msg.msg_type() {
        v6::MessageType::Solicit => build_advertise(msg, config),
        v6::MessageType::Request => build_reply(msg, config),
        _ => None,
    }
}
```

DHCPv6 Options sent:
- IA_NA with /128 address
- DNS servers
- NTP servers (if configured)

#### Router (`dataplane/router.rs`)

L3 routing between vNICs:

```rust
pub struct Router {
    // Network ID -> (IP prefix -> vNIC channel)
    routes: DashMap<String, RoutingTable>,
}

impl Router {
    pub fn lookup(&self, network_id: &str, dst: IpAddr) -> Option<Sender<RoutedPacket>>;
    pub fn add_route(&self, network_id: &str, prefix: IpNet, sender: Sender<RoutedPacket>);
    pub fn remove_nic(&self, network_id: &str, nic_id: &str);
}
```

Routing decision:
1. Look up destination IP in routing table for the network
2. Find longest prefix match
3. Send packet via crossbeam channel to target worker
4. Target worker injects packet into VM's RX vring

## Packet Flow Examples

### VM → ARP for Gateway

```
VM: ARP Who-has 169.254.0.1?
    │
    ▼
Worker: poll_rx() gets ARP packet
    │
    ▼
Worker: arp::handle_arp() recognizes gateway
    │
    ▼
Worker: builds ARP reply (169.254.0.1 is-at <gateway_mac>)
    │
    ▼
Worker: injects reply into VM's RX vring
    │
    ▼
VM: Receives ARP reply
```

### VM → DHCPv4

```
VM: DHCPDISCOVER (broadcast)
    │
    ▼
Worker: poll_rx() gets DHCP packet
    │
    ▼
Worker: dhcpv4::handle_dhcp() builds DHCPOFFER
    │  - yiaddr: 10.0.0.5
    │  - router: 169.254.0.1
    │  - subnet: 255.255.255.255
    │
    ▼
Worker: injects DHCPOFFER into VM's RX vring
    │
    ▼
VM: DHCPREQUEST
    │
    ▼
Worker: dhcpv4::handle_dhcp() builds DHCPACK
    │
    ▼
VM: Configures interface
```

### VM1 → VM2 (Inter-VM Routing)

```
VM1: IP packet to 10.0.0.6
    │
    ▼
Worker1: poll_rx() gets IP packet
    │
    ▼
Worker1: router.lookup("net1", 10.0.0.6)
    │  - Finds Worker2's channel
    │
    ▼
Worker1: channel.send(RoutedPacket)
    │
    ▼
Worker2: poll_router() receives packet
    │
    ▼
Worker2: Builds Ethernet frame with VM2's MAC
    │
    ▼
Worker2: injects into VM2's RX vring
    │
    ▼
VM2: Receives packet
```

## Public Networks and TUN Device

Networks can be configured as "public" (`is_public = true`), which enables internet access via a global TUN device.

### Architecture

```
┌────────────────────────────────────────────────────────────────────────────┐
│                              mvirt-net                                      │
│                                                                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐                         │
│  │ Private Net │  │ Public Net  │  │ Public Net  │                         │
│  │ is_public=0 │  │ is_public=1 │  │ is_public=1 │                         │
│  │             │  │             │  │             │                         │
│  │ VMs can     │  │ VMs get     │  │ VMs get     │                         │
│  │ only talk   │  │ default     │  │ default     │                         │
│  │ to each     │  │ route via   │  │ route via   │                         │
│  │ other       │  │ gateway     │  │ gateway     │                         │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘                         │
│         │                │                │                                 │
│         │ DROP           │ ToInternet     │ ToInternet                      │
│         │                │                │                                 │
│         ▼                └───────┬────────┘                                 │
│      (dropped)                   │                                          │
│                      ┌───────────▼───────────┐                              │
│                      │    TUN IO Thread      │                              │
│                      │    ("mvirt-net")       │                              │
│                      └───────────┬───────────┘                              │
│                                  │                                          │
└──────────────────────────────────┼──────────────────────────────────────────┘
                                   │
                                   ▼
                          ┌────────────────┐
                          │ Linux Kernel   │
                          │ - ip route     │
                          │ - iptables NAT │
                          │ - ip_forward   │
                          └────────┬───────┘
                                   │
                                   ▼
                               Internet
```

### TUN Device Lifecycle

1. **Startup**: TUN device "mvirt-net" is created when mvirt-net starts
2. **Route Management**: Routes are automatically added/removed when public networks are created/deleted
3. **Reconciliation**: A background loop (every 10s) ensures routes match the database state

### Route Management (`dataplane/tun.rs`)

```rust
// Routes are managed via `ip route` commands
pub fn add_route(subnet: &str) -> io::Result<()>;    // ip route add <subnet> dev mvirt-net
pub fn remove_route(subnet: &str) -> io::Result<()>; // ip route del <subnet> dev mvirt-net
pub fn get_routes() -> io::Result<Vec<String>>;      // ip route show dev mvirt-net
```

### Network Isolation

- **Private networks** (`is_public = false`):
  - Packets without a local destination are **dropped**
  - VMs can only communicate with other VMs in the same network
  - No default route announced via DHCP/RA

- **Public networks** (`is_public = true`):
  - Packets without a local destination go to TUN → Linux kernel
  - Default route (0.0.0.0/0, ::/0) announced via DHCP Router option and RA
  - IP address ranges must not overlap with other public networks

### DHCP/RA Behavior

| Setting | Private Network | Public Network |
|---------|-----------------|----------------|
| DHCP Router Option | Not sent | 169.254.0.1 |
| RA Router Lifetime | 0 (not a router) | 1800s |

### Host Configuration

The host must configure NAT for outbound traffic:

```bash
# Enable IP forwarding
echo 1 > /proc/sys/net/ipv4/ip_forward

# NAT for public network subnets
iptables -t nat -A POSTROUTING -s 10.200.0.0/24 -o eth0 -j MASQUERADE
```

## Memory Layout

### vhost-user Shared Memory

```
┌─────────────────────────────────────────────────────────────────┐
│                    Guest Memory Region                          │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Virtio-net Device                                        │   │
│  │                                                           │   │
│  │  ┌─────────────┐  ┌─────────────┐                        │   │
│  │  │  RX Vring   │  │  TX Vring   │                        │   │
│  │  │             │  │             │                        │   │
│  │  │ Descriptors │  │ Descriptors │                        │   │
│  │  │ Available   │  │ Available   │                        │   │
│  │  │ Used        │  │ Used        │                        │   │
│  │  └─────────────┘  └─────────────┘                        │   │
│  │                                                           │   │
│  │  ┌───────────────────────────────────────────────────┐   │   │
│  │  │              Packet Buffers                        │   │   │
│  │  └───────────────────────────────────────────────────┘   │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
           │
           │ mmap (shared with backend)
           ▼
┌─────────────────────────────────────────────────────────────────┐
│                    mvirt-net Process                            │
│                                                                  │
│  Worker Thread reads/writes directly to shared memory           │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## Dependencies

| Crate | Purpose |
|-------|---------|
| vhost | vhost-user protocol |
| vhost-user-backend | Backend framework |
| vm-memory | Guest memory abstraction |
| virtio-queue | Virtio queue handling |
| smoltcp | Packet parsing/building |
| dhcproto | DHCPv4/v6 protocol |
| crossbeam-channel | Inter-thread messaging |
| tonic | gRPC server |
| sqlx | SQLite async |

## Future Optimizations

### io_uring

Currently using blocking poll on vhost-user. Could use io_uring for:
- Batched notification handling
- Zero-copy packet processing
- Better CPU efficiency

### Hugepages

Currently using regular pages. Hugepages would:
- Reduce TLB misses
- Improve performance for large packet buffers
- Require system configuration

### Kernel Bypass

For highest performance, could bypass kernel entirely:
- DPDK-style memory management
- Direct NIC access for external connectivity
- Requires significant complexity increase
