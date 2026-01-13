# mvirt-cli

CLI und TUI Client für mvirt.

## Features

- **TUI** - Interaktive Oberfläche mit ratatui
- **CLI** - Scriptbare Kommandos
- **Console** - Serial Console Zugriff (Ctrl+a t zum Beenden)

## Verwendung

```bash
# TUI starten (default)
mvirt

# CLI Kommandos
mvirt list
mvirt create --name test --kernel /path/to/kernel --disk /path/to/disk.raw
mvirt start <id>
mvirt stop <id>
mvirt console <id>
```

## TUI

```
┌─────────────────────────────────────────────────────────┐
│ mvirt │ CPU 2/16 │ RAM 1.0/31.2 GiB                     │
├─────────────────────────────────────────────────────────┤
│ ID        NAME      STATE       CPU   MEM               │
│ a1b2c3d4… test-vm   ● running   2     1024MB            │
│ e5f6g7h8… other     ○ stopped   1     512MB             │
├─────────────────────────────────────────────────────────┤
│ ↵ Details  c Create  s Start  S Stop  k Kill  d Delete  │
└─────────────────────────────────────────────────────────┘
```

### Tastenkürzel

| Taste | Aktion |
|-------|--------|
| `↵` | VM-Details anzeigen |
| `c` | Neue VM erstellen |
| `s` | VM starten |
| `S` | VM stoppen (graceful) |
| `k` | VM killen (force) |
| `d` | VM löschen |
| `q` | Beenden |
| `↑/↓` | Navigation |

### Create Modal

- **Name** - Optional, nur `[a-zA-Z0-9-_]`
- **Kernel** - Pflicht, File Picker mit Enter
- **Disk** - Pflicht, File Picker mit Enter
- **VCPUs** - Default 1, nur Zahlen
- **Memory** - Default 512 MB, nur Zahlen
- **User-Data** - Optional, cloud-init YAML

## CLI Kommandos

### VM erstellen

```bash
mvirt create \
    --name myvm \
    --kernel /path/to/vmlinux \
    --disk /path/to/disk.raw \
    --vcpus 2 \
    --memory 1024 \
    --user-data /path/to/cloud-init.yaml
```

### VM verwalten

```bash
mvirt list                  # Alle VMs auflisten
mvirt get <id>              # VM-Details
mvirt start <id>            # VM starten
mvirt stop <id>             # Graceful Shutdown (30s Timeout)
mvirt stop <id> -t 60       # Mit 60s Timeout
mvirt kill <id>             # Force Kill
mvirt delete <id>           # VM löschen (muss gestoppt sein)
```

### Console

```bash
mvirt console <id>          # Serial Console verbinden
                            # Ctrl+a t zum Beenden
```

## Optionen

| Option | Default | Beschreibung |
|--------|---------|--------------|
| `-s, --server` | `http://[::1]:50051` | gRPC Server-Adresse |

## Exit Codes

| Code | Bedeutung |
|------|-----------|
| 0 | Erfolg |
| 1 | Fehler (Connection, API, etc.) |
