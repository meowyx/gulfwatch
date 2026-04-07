# GulfWatch

Real-time observability and alerting for Solana programs. Streams live transaction data, computes metrics, fires alerts, and presents everything through a terminal TUI and REST API.

## Structure

```
crates/
  gulfwatch-core/       # shared types, rolling window, metrics, alerts, pipeline
  gulfwatch-ingest/     # Solana WebSocket RPC client, transaction parsing
  gulfwatch-server/     # axum REST API + WebSocket + Prometheus (binary)
  gulfwatch-tui/        # terminal dashboard (binary)
web/                    # Next.js dashboard (frontend)
```

## Setup

```bash
git clone https://github.com/meowyx/gulfwatch.git
cd gulfwatch
```

Create a `.env` file in the project root:

```
SOLANA_WS_URL=wss://api.devnet.solana.com
SOLANA_RPC_URL=https://api.devnet.solana.com
MONITOR_PROGRAM=TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA
```

Or with a Helius API key for better rate limits:

```
SOLANA_WS_URL=wss://devnet.helius-rpc.com/?api-key=YOUR_KEY
SOLANA_RPC_URL=https://devnet.helius-rpc.com/?api-key=YOUR_KEY
MONITOR_PROGRAM=TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA
```

## Running

### TUI (terminal dashboard)

Self-contained — connects directly to Solana, no server needed.

```bash
cargo run -p gulfwatch-tui
```

**Keybindings:**

| Key | Action |
|---|---|
| `Tab` / `Shift-Tab` | Switch panels |
| `1` `2` `3` | Jump to panel |
| `j`/`k` or `Up`/`Down` | Scroll / move selection |
| `Enter` | Open detail view |
| `Esc` / `Backspace` | Back to dashboard |
| `q` / `Ctrl-C` | Quit |

### Server (REST API + WebSocket)

For the web dashboard frontend. Optionally set `LISTEN_ADDR` (defaults to `0.0.0.0:3001`).

```bash
cargo run -p gulfwatch-server
```

The server supports comma-separated programs via `MONITOR_PROGRAMS`:

```
MONITOR_PROGRAMS=675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8,JUP6LkMUjV1hTVo8YS7ZMCwnvzKmqPuqZoFkMjEHpKu
```

### Tests

```bash
cargo test --workspace
```

## API

### REST

```
GET  /health
GET  /api/programs
POST /api/programs                  { "program_id": "..." }
DELETE /api/programs/{id}
GET  /api/metrics/summary           ?program=...
GET  /api/metrics/timeseries        ?program=...&interval=60
GET  /api/transactions/recent       ?program=...&limit=50
GET  /api/alerts
POST /api/alerts                    { AlertRule JSON }
PUT  /api/alerts/{id}
DELETE /api/alerts/{id}
GET  /metrics                       Prometheus format
```

### WebSocket

```
WS /ws/feed

Client sends:    { "subscribe": ["program_id"] }
                 { "unsubscribe": ["program_id"] }

Server sends:    { "type": "transaction", "data": { ... } }
                 { "type": "alert", "data": { ... } }
```
