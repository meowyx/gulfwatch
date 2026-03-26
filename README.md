# GulfWatch

Real-time observability and alerting for Solana programs. Streams live transaction data via Yellowstone gRPC, computes metrics, fires alerts, and serves it all through a terminal TUI and web dashboard.

## Stack

- **Backend:** Rust (axum, tonic, tokio, ratatui)
- **Frontend:** Next.js
- **Data source:** Yellowstone gRPC

## Structure

```
crates/
  gulfwatch-core/       # shared types and config 
  gulfwatch-ingest/     # gRPC client, transaction parsing -WIP
  gulfwatch-server/     # axum server (REST + WebSocket + Prometheus) - WIP
  gulfwatch-tui/        # terminal UI - WIP
web/                    # Next.js dashboard
```
