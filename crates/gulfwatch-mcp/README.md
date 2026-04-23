# gulfwatch-mcp

MCP server that exposes the GulfWatch REST surface as tools for Claude Code (or any MCP client). Read-only in v1.

## Tools

| Tool | Wraps | What it returns |
|---|---|---|
| `recent_transactions` | `GET /api/transactions/recent` | Decoded txs from the rolling window with classification filters |
| `get_transaction` | `GET /api/transactions/{signature}` | Full decoded tx (instructions, logs, balance_diff, tx_error, cu_profile). Works for **any** mainnet tx — falls back to Solana RPC if not in the rolling window. |
| `metrics_summary` | `GET /api/metrics/summary` | Tx count, error rate, avg CU, top instruction types per program |
| `list_programs` | `GET /api/programs` | Monitored program ids |
| `list_alert_rules` | `GET /api/alerts` | Configured threshold alert rules |
| `recent_alerts` | `GET /api/alerts/recent` | Recent alert fires (security detections + threshold rules), newest first |

## Install

Build the binary:

```bash
cargo build -p gulfwatch-mcp --release
```

Then either use the Claude Code CLI (recommended):

```bash
claude mcp add --scope user \
  --transport stdio \
  --env GULFWATCH_BASE_URL=http://localhost:3001 \
  gulfwatch -- /absolute/path/to/gulfwatch/target/release/gulfwatch-mcp
```

Or commit a project-scoped `.mcp.json` at the repo root (preferred for open-source — anyone who clones picks it up automatically):

```json
{
  "mcpServers": {
    "gulfwatch": {
      "type": "stdio",
      "command": "/absolute/path/to/gulfwatch/target/release/gulfwatch-mcp",
      "env": { "GULFWATCH_BASE_URL": "http://localhost:3001" }
    }
  }
}
```

Restart Claude Code (full quit + relaunch — no hot reload for stdio servers). Run `/mcp` in a new session to confirm `gulfwatch` shows connected.

## Prereqs

A running GulfWatch instance reachable at `GULFWATCH_BASE_URL` (default `http://localhost:3001`). Either:

- `cargo run -p gulfwatch` — TUI with the HTTP surface embedded. Recommended: one process, you also get the visual dashboard.
- `cargo run -p gulfwatch-server` — headless. Useful if you don't want a TUI taking over a terminal.

## Demo flow

With the server running and the MCP wired in, ask Claude things like:

- "look up signature `5VGz...` and tell me why it failed"
- "what alerts fired in the last hour on raydium"
- "is anyone moving large amounts out of my watched accounts right now"
- "show me 20 txs that failed with custom error code 6004"

## Env vars

| Variable | Default | What it does |
|---|---|---|
| `GULFWATCH_BASE_URL` | `http://localhost:3001` | HTTP base for the GulfWatch server |
| `RUST_LOG` | `info` | Logging via `tracing-subscriber`'s `EnvFilter` |
