# mvirt-ui

React-based web console for mvirt with mock backend.

## Quick Start

```bash
# Terminal 1: Start mock server
cd mvirt-ui && cargo run -p mock-server

# Terminal 2: Start dev server
cd mvirt-ui && npm run dev
```

Then open http://localhost:3000 in your browser.

## Tech Stack

- **Frontend**: React 18 + Vite + TypeScript
- **Styling**: Tailwind CSS + shadcn/ui (Radix-based)
- **State**: TanStack Query (server state) + Zustand (UI state)
- **Terminal**: xterm.js
- **Charts**: recharts
- **Mock Server**: Rust + Axum

## Project Structure

```
mvirt-ui/
├── mock-server/           # Rust Axum mock server
│   ├── src/
│   │   ├── main.rs        # Server setup
│   │   ├── state.rs       # In-memory state + mock data
│   │   └── routes/        # Route handlers
├── src/
│   ├── api/               # API client + endpoints
│   ├── types/             # TypeScript interfaces
│   ├── hooks/             # TanStack Query hooks
│   ├── components/
│   │   ├── ui/            # shadcn/ui components
│   │   ├── layout/        # Sidebar, Header, Layout
│   │   └── data-display/  # DataTable, StatCard, StateIndicator
│   └── features/          # Page components
│       ├── dashboard/
│       ├── vms/
│       ├── storage/
│       ├── network/
│       └── logs/
```

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/v1/vms` | GET/POST | List/Create VMs |
| `/api/v1/vms/:id` | GET/DELETE | Get/Delete VM |
| `/api/v1/vms/:id/start` | POST | Start VM |
| `/api/v1/vms/:id/stop` | POST | Stop VM |
| `/api/v1/vms/:id/console` | WebSocket | Serial console |
| `/api/v1/events/vms` | SSE | VM state events |
| `/api/v1/storage/volumes` | GET/POST | List/Create volumes |
| `/api/v1/storage/templates` | GET | List templates |
| `/api/v1/networks` | GET/POST | List/Create networks |
| `/api/v1/nics` | GET/POST | List/Create NICs |
| `/api/v1/logs` | GET | Query logs |
| `/api/v1/logs/stream` | SSE | Live log tail |
| `/api/v1/system` | GET | System info |

## Development

```bash
# Install dependencies
npm install

# Type check
npx tsc --noEmit

# Build for production
npm run build

# Build mock server
cargo build -p mock-server
```

## Design

- Dark mode default
- Monospace font (JetBrains Mono) for IDs, paths, IPs
- State colors: Running (green), Starting (yellow), Stopped (gray), Error (red)
- Real-time updates via SSE
