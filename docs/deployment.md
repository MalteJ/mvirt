# Deployment

Guide for running mvirt in production.

## System Requirements

- Linux kernel with KVM support (`/dev/kvm`)
- x86_64 architecture
- ZFS (optional, for mvirt-zfs)
- Root access for networking (mvirt-net)

### Minimum Resources

| VMs | RAM   | CPU | Disk   |
|-----|-------|-----|--------|
| 1-5 | 4 GB  | 2   | 50 GB  |
| 5-20| 16 GB | 4   | 200 GB |
| 20+ | 32 GB | 8   | 500 GB |

*Plus resources for VMs themselves.*

## Installation

### From Source

```bash
# Build static binaries
make release

# Copy to system
sudo cp target/x86_64-unknown-linux-musl/release/mvirt /usr/bin/
sudo cp target/x86_64-unknown-linux-musl/release/mvirt-vmm /usr/sbin/
sudo cp target/x86_64-unknown-linux-musl/release/mvirt-log /usr/sbin/
sudo cp target/x86_64-unknown-linux-musl/release/mvirt-net /usr/sbin/
sudo cp target/x86_64-unknown-linux-musl/release/mvirt-zfs /usr/sbin/
sudo cp target/x86_64-unknown-linux-musl/release/cloud-hypervisor /usr/bin/
```

### Directory Setup

```bash
# Create data directories
sudo mkdir -p /var/lib/mvirt/vmm
sudo mkdir -p /var/lib/mvirt/log
sudo mkdir -p /var/lib/mvirt/net
sudo mkdir -p /run/mvirt/vmm
sudo mkdir -p /run/mvirt/net
```

## Service Configuration

### systemd Units

Create `/etc/systemd/system/mvirt-vmm.service`:

```ini
[Unit]
Description=mvirt VM Manager
After=network.target

[Service]
Type=simple
ExecStart=/usr/sbin/mvirt-vmm --data-dir /var/lib/mvirt/vmm
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Create `/etc/systemd/system/mvirt-log.service`:

```ini
[Unit]
Description=mvirt Logging Service
After=network.target

[Service]
Type=simple
ExecStart=/usr/sbin/mvirt-log --data-dir /var/lib/mvirt/log
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Create `/etc/systemd/system/mvirt-net.service`:

```ini
[Unit]
Description=mvirt Networking Service
After=network.target

[Service]
Type=simple
ExecStart=/usr/sbin/mvirt-net --socket-dir /run/mvirt/net --metadata-dir /var/lib/mvirt/net
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Create `/etc/systemd/system/mvirt-zfs.service`:

```ini
[Unit]
Description=mvirt ZFS Storage Service
After=zfs.target

[Service]
Type=simple
ExecStart=/usr/sbin/mvirt-zfs --pool vmpool
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

### Enable Services

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now mvirt-log
sudo systemctl enable --now mvirt-vmm
sudo systemctl enable --now mvirt-zfs   # If using ZFS
sudo systemctl enable --now mvirt-net   # If using networking
```

## ZFS Pool Setup

```bash
# Create a pool (example with single disk)
sudo zpool create vmpool /dev/sdX

# Or with mirror
sudo zpool create vmpool mirror /dev/sdX /dev/sdY

# Enable compression (recommended)
sudo zfs set compression=lz4 vmpool
```

## Network Bridge Setup

mvirt-net creates TAP devices. Set up a bridge for external connectivity:

```bash
# Create bridge
sudo ip link add br0 type bridge
sudo ip link set br0 up

# Add physical interface (optional, for external access)
sudo ip link set eth0 master br0

# Configure IP on bridge
sudo ip addr add 192.168.1.1/24 dev br0
```

## Firewall

Open gRPC ports if accessing remotely:

```bash
# UFW example
sudo ufw allow 50051/tcp  # mvirt-vmm
sudo ufw allow 50052/tcp  # mvirt-log
sudo ufw allow 50053/tcp  # mvirt-zfs
sudo ufw allow 50054/tcp  # mvirt-net
```

## Monitoring

### Service Status

```bash
systemctl status mvirt-vmm mvirt-log mvirt-zfs mvirt-net
```

### Logs

```bash
journalctl -u mvirt-vmm -f
journalctl -u mvirt-log -f
```

### Health Check

```bash
# Check if services are responding
grpcurl -plaintext [::1]:50051 list
grpcurl -plaintext [::1]:50052 list
grpcurl -plaintext [::1]:50053 list
grpcurl -plaintext [::1]:50054 list
```

## Troubleshooting

### VM won't start

1. Check KVM access: `ls -la /dev/kvm`
2. Check kernel path exists
3. Run with debug logging: `RUST_LOG=debug mvirt-vmm`

### ZFS errors

1. Check pool status: `zpool status vmpool`
2. Check permissions: ZFS requires root
3. Check pool exists: `zpool list`

### Networking issues

1. Check TAP device: `ip link show | grep tap`
2. Check bridge: `bridge link show`
3. mvirt-net requires root for TAP creation

## See Also

- [Architecture](architecture.md) - System design
- [Data Directories](reference/data-directories.md) - Storage locations
- [Service Ports](reference/ports.md) - Port configuration
