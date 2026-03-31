# GulfWatch

Real-time observability and alerting for Solana programs. Streams live transaction data, computes metrics, fires alerts, and serves it all through a terminal TUI and web dashboard.

## Stack

- **Backend:** Rust (axum, tokio, ratatui)
- **Frontend:** Next.js
- **Data source:** Solana WebSocket RPC (Phase 1), Yellowstone gRPC (Phase 2)

## Structure

```
crates/
  gulfwatch-core/       # shared types, rolling window, metrics
  gulfwatch-ingest/     # transaction ingestion - WIP
  gulfwatch-server/     # axum server (REST + WebSocket + Prometheus) - WIP
  gulfwatch-tui/        # terminal UI - WIP
web/                    # Next.js dashboard
```

## Setup

```bash
git clone https://github.com/meowyx/gulfwatch.git
cd gulfwatch
cargo check
cargo test
```

## What's built so far

`gulfwatch-core` contains the foundation:

- **Transaction** - core data type for parsed Solana transactions
- **MetricSummary / InstructionCount** - computed metrics shapes for the REST API
- **RollingWindow** - time-limited in-memory buffer that stores transactions, auto-evicts expired entries, and computes metrics on demand
