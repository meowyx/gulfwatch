# Transaction Classification

How GulfWatch assigns a high-level transaction type (`swap`, `bridge_out`, `stake_withdraw`, etc.) after parsing instructions.

If you remember one sentence: **parser classification labels each instruction, transaction classification labels the whole transaction.**

## Where this runs

- Input comes from `gulfwatch-ingest::parser` as a typed `Transaction` with `instructions`.
- `gulfwatch-core::pipeline::run_processing_worker` maps parsed instructions into `gulfwatch_classification::InstructionInput` and calls `ClassificationService`.
- Output is attached to every transaction:
  - `Transaction.classification`
  - `Transaction.classification_debug`

Key files:

- `crates/gulfwatch-classification/src/lib.rs`
- `crates/gulfwatch-core/src/pipeline.rs`
- `crates/gulfwatch-core/src/transaction.rs`

## Current classifier chain

Classifiers run in priority order (first match wins):

1. `authority-change`
2. `failed-transaction`
3. `solana-pay`
4. `bridge`
5. `privacy-cash`
6. `nft-mint`
7. `nft-transfer`
8. `stake-deposit`
9. `stake-withdraw`
10. `swap`
11. `airdrop`
12. `fee-only`
13. `transfer`
14. `fallback`

The selected result is stored as:

- `classification.classifier` (which rule won)
- `classification.category` (semantic type)
- `classification.confidence`
- `classification.summary`

## Debug trace

`classification_debug` is intentionally verbose for debugging:

- `focal_account`
- `decisions[]` (each classifier, matched/skipped reason)
- `legs[]` (derived transfer legs, direction, amount)

This is what the TUI detail view uses to explain *why* a tx got a given label.

## Program ID source of truth

Program IDs are currently kept as local constants, versioned in repo:

- Transaction classifiers: `crates/gulfwatch-classification/src/program_ids/mod.rs`
- Ingest parser decoding: `crates/gulfwatch-ingest/src/program_ids/mod.rs`

So today they are **not fetched from network** and **not stored in a DB**; they ship with the binary.

## API/TUI behavior

- TUI transaction list shows classifier type and falls back to instruction headline when classifier is `fallback`.
- REST `GET /api/transactions/recent` supports filters:
  - `category`
  - `classifier`
  - `min_confidence`
  - `has_debug`

## Known follow-ups

- Share parser/classifier program IDs from one crate to avoid duplicate constant sets.
- Port tx-indexer parity fixtures into GulfWatch classification tests.
- Optionally move IDs to config file/env once operational needs require runtime updates.
