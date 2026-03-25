# all the crates will be here

```

  ├── crates/
  │   ├── gulfwatch-core/      # shared types, models, config
  │   │   ├── Cargo.toml
  │   │   └── src/
  │   │       └── lib.rs
  │   │
  │   ├── gulfwatch-ingest/    # gRPC client, transaction parsing
  │   │   ├── Cargo.toml
  │   │   └── src/
  │   │       └── lib.rs
  │   │
  │   ├── gulfwatch-server/    # axum server (REST + WebSocket + Prometheus)
  │   │   ├── Cargo.toml
  │   │   └── src/
  │   │       └── main.rs
  │   │
  │   └── gulfwatch-tui/       # ratatui terminal interface
  │       ├── Cargo.toml
  │       └── src/
  │           └── main.rs

```