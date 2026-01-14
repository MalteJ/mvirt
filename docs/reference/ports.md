# Service Ports

All mvirt services use gRPC over IPv6 localhost by default.

## Port Assignments

| Service    | Port  | Default Address   | Proto File                  |
|------------|-------|-------------------|-----------------------------|
| mvirt-vmm  | 50051 | `[::1]:50051`     | `mvirt-vmm/proto/mvirt.proto` |
| mvirt-log  | 50052 | `[::1]:50052`     | `mvirt-log/proto/log.proto`   |
| mvirt-zfs  | 50053 | `[::1]:50053`     | `mvirt-zfs/proto/zfs.proto`   |
| mvirt-net  | 50054 | `[::1]:50054`     | `mvirt-net/proto/net.proto`   |

## Customizing Ports

Each daemon accepts `--listen` to change the listen address:

```bash
# Listen on all interfaces
mvirt-vmm --listen 0.0.0.0:50051

# Custom port
mvirt-zfs --pool vmpool --listen [::1]:9000
```

## Client Configuration

The CLI connects to all services. Override with flags:

```bash
mvirt --server http://[::1]:50051 \
      --log-server http://[::1]:50052 \
      --zfs-server http://[::1]:50053 \
      --net-server http://[::1]:50054
```

## Inter-Service Communication

Services log to mvirt-log via `--log-endpoint`:

```bash
mvirt-vmm --log-endpoint http://[::1]:50052
mvirt-zfs --pool vmpool --log-endpoint http://[::1]:50052
mvirt-net --log-endpoint http://[::1]:50052
```
