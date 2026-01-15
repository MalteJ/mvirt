# Networking in mvirt

`mvirt` provides Layer 3 networking for virtual machines through **mvirt-net**, a dedicated network daemon that implements a vhost-user backend for virtio-net devices.

## Concepts

### Networks

A **Network** is an isolated routing domain, similar to a VPC (Virtual Private Cloud) in public clouds. VMs in different networks cannot communicate with each other by default.

Each network can be configured as:
- **IPv4-only**: VMs receive IPv4 addresses only
- **IPv6-only**: VMs receive IPv6 addresses only
- **Dual-Stack**: VMs receive both IPv4 and IPv6 addresses

Networks support per-network configuration of:
- DNS servers (announced via DHCP)
- NTP servers (announced via DHCP)

### vNICs (Virtual Network Interface Cards)

A **vNIC** connects a VM to a network. Each vNIC:

- Belongs to exactly one network
- Has a unique MAC address (auto-generated or user-specified)
- Receives IP addresses via DHCP (not statically configured inside the VM)
- Can optionally have routed prefixes for advanced use cases

### IP Address Assignment

Unlike traditional bridged networking, `mvirt-net` uses a pure Layer 3 model:

| Property | IPv4 | IPv6 |
|----------|------|------|
| Address per vNIC | Single /32 | Single /128 |
| Gateway | 169.254.0.1 | fe80::1 |
| Assignment | DHCPv4 | DHCPv6 (with RA M=1, O=1) |

**Why /32 and /128 addresses?**

Each vNIC receives a point-to-point address. There is no Layer 2 network segment between VMs - all traffic is routed through `mvirt-net`. This provides:

1. **Better isolation**: VMs cannot sniff each other's traffic
2. **Simplified networking**: No ARP/NDP between VMs, no broadcast storms
3. **Flexibility**: VMs can move between hosts without L2 dependencies

### Routed Prefixes

For VMs that act as routers (e.g., running containers, nested VMs, or VPN endpoints), additional prefixes can be routed to a vNIC:

```
vNIC Address:     10.0.0.5/32
Routed Prefixes:  10.0.1.0/24, 10.0.2.0/24
```

Traffic destined for `10.0.1.0/24` or `10.0.2.0/24` will be forwarded to this vNIC, allowing the VM to route it internally.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                           mvirt-net                              │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐           │
│  │   vNIC #1    │  │   vNIC #2    │  │   vNIC #N    │           │
│  │              │  │              │  │              │           │
│  │  ARP/NDP     │  │  ARP/NDP     │  │  ARP/NDP     │           │
│  │  DHCPv4/v6   │  │  DHCPv4/v6   │  │  DHCPv4/v6   │           │
│  │  Router      │  │  Router      │  │  Router      │           │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘           │
│         │                 │                 │                    │
│         └────────────┬────┴─────────────────┘                    │
│                      │                                           │
│              L3 Routing Table                                    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
          │                 │                 │
    vhost-user        vhost-user        vhost-user
          │                 │                 │
    ┌─────▼─────┐     ┌─────▼─────┐     ┌─────▼─────┐
    │   VM #1   │     │   VM #2   │     │   VM #N   │
    └───────────┘     └───────────┘     └───────────┘
```

## Workflow

### 1. Create a Network

```bash
mvirt network create --name production \
    --ipv4-subnet 10.0.0.0/24 \
    --ipv6-prefix fd00::/64 \
    --dns 1.1.1.1 --dns 2606:4700:4700::1111
```

### 2. Create a vNIC

```bash
mvirt nic create --network production --name web-01-eth0
# Returns: nic-abc123 with socket /run/mvirt/net/nic-abc123.sock
```

### 3. Start VM with vNIC

```bash
mvirt vm start web-01 --nic nic-abc123
```

### 4. VM Boot Process

1. VM kernel initializes virtio-net device
2. VM sends DHCP Discover
3. `mvirt-net` responds with DHCP Offer (IP: 10.0.0.X/32, Router: 169.254.0.1)
4. VM configures interface
5. VM can now communicate with other VMs in the same network

## Gateway Addressing

The gateway addresses are link-local and identical for all vNICs:

| Protocol | Gateway Address |
|----------|-----------------|
| IPv4 | 169.254.0.1 |
| IPv6 | fe80::1 |

`mvirt-net` responds to ARP requests for 169.254.0.1 and Neighbor Solicitations for fe80::1, presenting a virtual MAC address. All outbound traffic from VMs is sent to this gateway and then routed by `mvirt-net`.

## Protocol Details

### IPv4 (DHCPv4)

1. VM broadcasts DHCPDISCOVER
2. `mvirt-net` responds with DHCPOFFER:
   - Your IP: `<assigned>/32`
   - Router: `169.254.0.1`
   - DNS: `<configured servers>`
   - Lease time: Infinite (static assignment)
3. VM sends DHCPREQUEST
4. `mvirt-net` confirms with DHCPACK

### IPv6 (SLAAC + DHCPv6)

1. `mvirt-net` sends periodic Router Advertisements:
   - M flag = 1 (Managed address configuration)
   - O flag = 1 (Other configuration)
   - Router lifetime > 0
   - Prefix information (for on-link determination only)
2. VM sends DHCPv6 Solicit
3. `mvirt-net` responds with DHCPv6 Advertise
4. VM sends DHCPv6 Request
5. `mvirt-net` confirms with DHCPv6 Reply:
   - IA_NA with assigned /128 address
   - DNS servers
   - NTP servers (if configured)

## Isolation and Security

- **Network isolation**: Traffic cannot cross network boundaries
- **No L2 exposure**: VMs cannot see each other's MAC addresses or sniff traffic
- **Controlled ARP/NDP**: Only gateway addresses are resolved
- **No IP spoofing**: `mvirt-net` only accepts packets from assigned addresses

## Future Features

- **External connectivity**: NAT/Masquerade for outbound internet access
- **Floating IPs**: Public IPs that can be moved between vNICs
- **Network policies**: Firewall rules between networks
- **Load balancing**: L4 load balancer for services
